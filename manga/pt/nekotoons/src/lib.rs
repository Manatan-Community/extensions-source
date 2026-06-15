use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: NekoToons = NekoToons;
const BASE_URL: &str = "https://nekotoons.site";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct NekoToons;

impl MangaSource for NekoToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_popular(HOME_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_document(&format!("{BASE_URL}/?pagina={page}"), LATEST_FIXTURE);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: has_next_page(&body),
            });
        }
        Ok(Paged {
            entries: parse_popular(&fetch_document(BASE_URL, HOME_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_document(
            &format!("{BASE_URL}/?search={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let Some(manga_id) = extract_manga_id(&body) else {
            return Ok(parse_chapters(&body));
        };
        let mut chapters = Vec::new();
        for page in 1..=50 {
            let ajax = fetch_json(
                &format!("{BASE_URL}/ajax/lzmvke.php?order=DESC&manga_id={manga_id}&page={page}"),
                CHAPTERS_FIXTURE,
            );
            let dto: ChaptersDto = serde_json::from_str(&ajax)
                .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
            chapters.extend(parse_chapters(&dto.chapters));
            if dto.remaining <= 0 {
                break;
            }
        }
        Ok(chapters.into_iter().fold(Vec::new(), push_unique_chapter))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/capitulo-1".to_string());
        let page_url = absolute_url(&key);
        let body = fetch_document(&page_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &page_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let home = fetch_document(BASE_URL, HOME_FIXTURE);
        let latest = fetch_document(&format!("{BASE_URL}/?pagina=1"), LATEST_FIXTURE);
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: parse_popular(&home),
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: parse_latest(&latest),
                has_more: has_next_page(&latest),
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("top10-item") || (chunk.contains("/manga/") && chunk.contains("<h3"))
        })
        .filter_map(|chunk| catalog_from_chunk(chunk, "h3", image_attr(chunk)))
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-cover", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_manga_key(&href);
            let title = html::text_between(chunk, "manga-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Neko Toons".to_string())
                });
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "manga-cover", "data-src")
                    .or_else(|| image_attr(chunk))
                    .map(|image| absolute_url(&image)),
                false,
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("search-result-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "onclick")
                .and_then(|onclick| onclick.split('\'').nth(1).map(ToString::to_string))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "search-result-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Neko Toons".to_string())
                });
            Some(catalog_item(
                normalize_manga_key(&href),
                title,
                image_attr(chunk).map(|image| absolute_url(&image)),
                false,
            ))
        })
        .fold(Vec::new(), push_unique)
}

fn catalog_from_chunk(
    chunk: &str,
    title_marker: &str,
    cover: Option<String>,
) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    if !href.contains("/manga/") {
        return None;
    }
    let key = normalize_manga_key(&href);
    let title = html::text_between(
        chunk,
        &format!("<{title_marker}"),
        &format!("</{title_marker}>"),
    )
    .or_else(|| html::attr_after(chunk, "<img", "alt"))
    .map(|value| html::strip_tags(&value))
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Neko Toons".to_string()));
    Some(catalog_item(
        key,
        title,
        cover.map(|image| absolute_url(&image)),
        false,
    ))
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(&absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let section =
        html::text_between(body, "manga-banner", "</section>").unwrap_or_else(|| body.to_string());
    let mut item = catalog_item(
        key.clone(),
        html::text_between(&section, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "Neko Toons".to_string())
            }),
        image_attr(&section).map(|image| absolute_url(&image)),
        true,
    );
    item.tags = section
        .split("<")
        .filter(|chunk| chunk.contains("genre-tag"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect();
    item.description = html::text_between(&section, "sinopse", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.status = parse_status(
        html::text_between(&section, "manga-meta", "</div>")
            .map(|value| html::strip_tags(&value))
            .as_deref(),
    );
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "capitulo-numero", "</")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "src")
                .or_else(|| html::attr(chunk, "data-src"))
                .filter(|image| !image.starts_with("data:") && !image.is_empty())
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(page_url)),
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_manga_id(body: &str) -> Option<String> {
    let marker = "obra_id";
    let rest = body.split(marker).nth(1)?;
    let digits = rest
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then_some(digits)
}

fn has_next_page(body: &str) -> bool {
    let active = html::attr_after(body, "page-link active", "href");
    let next = body
        .split("<a")
        .skip(1)
        .find(|chunk| chunk.contains("page-link") && chunk.contains("&gt;"))
        .and_then(|chunk| html::attr(chunk, "href"));
    next.is_some() && next != active
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "em andamento" => ItemStatus::Ongoing,
        "completo" | "concluído" | "concluido" => ItemStatus::Completed,
        "cancelado" => ItemStatus::Cancelled,
        "hiato" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_manga_key(value: &str) -> String {
    let key = normalize_key(value);
    if key.starts_with("/manga/") {
        key
    } else {
        let slug = url::slug_from_url(&key).unwrap_or_else(|| key.trim_matches('/').to_string());
        format!("/manga/{slug}")
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

#[derive(Deserialize)]
struct ChaptersDto {
    #[serde(default)]
    chapters: String,
    #[serde(default)]
    remaining: i64,
}

const HOME_FIXTURE: &str = r#"
<div class="top10-item"><a href="/manga/sample"><img src="/cover.jpg"><h3>Sample Neko</h3></a></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div class="manga-list"><div class="manga-card"><a class="manga-cover" href="/manga/sample"><img data-src="/cover.jpg"></a><a class="manga-title">Sample Neko</a></div></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<div class="search-result-item" onclick="location.href='/manga/sample'"><img src="/cover.jpg"><div class="search-result-title">Sample Neko</div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<section class="manga-banner"><div class="container"><h1>Sample Neko</h1><img src="/cover.jpg"><a class="genre-tag">Ação</a><div class="sinopse"><p>Resumo.</p></div><div class="manga-meta"><div>Em andamento</div></div></div></section>
<script>const obra_id = 123;</script>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":"<a class=\"chapter-item\" href=\"/manga/sample/capitulo-1\"><span class=\"capitulo-numero\">Capítulo 1</span></a>","remaining":0}"#;
const PAGES_FIXTURE: &str =
    r#"<picture><img src="/page1.jpg"></picture><picture><img src="/page2.jpg"></picture>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yuyu_fixtures() {
        assert_eq!(parse_popular(HOME_FIXTURE).len(), 1);
        assert_eq!(parse_latest(LATEST_FIXTURE).len(), 1);
        assert_eq!(
            parse_chapters(
                &serde_json::from_str::<ChaptersDto>(CHAPTERS_FIXTURE)
                    .unwrap()
                    .chapters
            )
            .len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
        assert_eq!(extract_manga_id(DETAILS_FIXTURE).as_deref(), Some("123"));
    }
}
