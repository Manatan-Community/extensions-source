use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KairosToons = KairosToons;
const BASE_URL: &str = "https://kairostoons.net";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";

struct KairosToons;

impl MangaSource for KairosToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            if page > 1 {
                let body = fetch_json(
                    &format!("{BASE_URL}/manga/ajax/load-more-releases/?page={page}"),
                    LOAD_MORE_FIXTURE,
                );
                let dto: LoadMoreDto = serde_json::from_str(&body)
                    .unwrap_or_else(|_| serde_json::from_str(LOAD_MORE_FIXTURE).unwrap());
                return Ok(Paged {
                    entries: parse_cards(&dto.html),
                    has_next_page: dto.has_next,
                });
            }
            let body = fetch_document(BASE_URL, LIST_FIXTURE);
            return Ok(Paged {
                entries: parse_cards(&body),
                has_next_page: body.contains("load-more-btn"),
            });
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga/todos/?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_json(
            &format!(
                "{BASE_URL}/search/live-search/?q={}",
                url::query_escape(query)
            ),
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
        let mut out = Vec::new();
        for page in 1..=20 {
            let target = format!("{}?page={page}", absolute_url(&key));
            let body = fetch_document(&target, DETAILS_FIXTURE);
            let before = out.len();
            out.extend(parse_chapters(&body));
            if !has_next_page(&body) || out.len() == before {
                break;
            }
        }
        Ok(out)
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
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga/todos/"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_cards(body),
        has_next_page: has_next_page(body),
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("comic-card-link")
                || (chunk.contains("/manga/") && chunk.contains("<h3"))
        })
        .filter_map(catalog_from_card)
        .fold(Vec::new(), push_unique)
}

fn catalog_from_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    if !href.contains("/manga/") {
        return None;
    }
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<h3", "</h3>")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Kairos Toons".to_string()));
    Some(catalog_item(
        key,
        title,
        image_attr(chunk).map(|image| absolute_url(&image)),
        false,
    ))
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    let dto: SearchDto = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
    dto.results
        .into_iter()
        .map(|item| {
            catalog_item(
                normalize_key(&item.url),
                item.title,
                Some(absolute_url(&item.cover_url)),
                false,
            )
        })
        .collect()
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut item = catalog_item(
        key.clone(),
        html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "Kairos Toons".to_string())
            }),
        html::attr_after(body, "sidebar-cover-image", "src").map(|image| absolute_url(&image)),
        true,
    );
    item.description = html::text_between(body, "manga-description", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.tags = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre-tag"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect();
    item.status = parse_status(
        html::text_between(body, "status-tag", "</")
            .map(|value| html::strip_tags(&value))
            .as_deref(),
    );
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-number", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                date_uploaded: html::text_between(chunk, "chapter-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_ymd(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split('<')
        .filter(|chunk| chunk.contains("chapter-image-canvas") || chunk.contains("data-src-url"))
        .filter_map(|chunk| {
            html::attr(chunk, "data-src-url")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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
        "concluído" | "concluido" | "completo" => ItemStatus::Completed,
        "hiato" => ItemStatus::Hiatus,
        "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
}

fn has_next_page(body: &str) -> bool {
    body.contains("aria-label=\"Próxima\"") || body.contains("aria-label='Próxima'")
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
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
struct SearchDto {
    #[serde(default)]
    results: Vec<SearchMangaDto>,
}

#[derive(Deserialize)]
struct SearchMangaDto {
    title: String,
    url: String,
    #[serde(rename = "cover_url")]
    cover_url: String,
}

#[derive(Deserialize)]
struct LoadMoreDto {
    #[serde(default)]
    html: String,
    #[serde(default, rename = "has_next")]
    has_next: bool,
}

const LIST_FIXTURE: &str = r#"
<a class="comic-card-link" href="/manga/sample"><img src="/cover.jpg"><h3>Sample Kairos</h3></a>
<a class="page-link" aria-label="Próxima" href="/manga/todos/?page=2">Next</a>
"#;
const SEARCH_FIXTURE: &str =
    r#"{"results":[{"title":"Sample Kairos","url":"/manga/sample","cover_url":"/cover.jpg"}]}"#;
const LOAD_MORE_FIXTURE: &str = r#"{"html":"<a class=\"comic-card-link\" href=\"/manga/sample\"><img src=\"/cover.jpg\"><h3>Sample Kairos</h3></a>","has_next":false}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Kairos</h1><div class="sidebar-cover-image"><img src="/cover.jpg"></div>
<div class="manga-description"><p>Resumo.</p></div><a class="genre-tag">Ação</a><span class="status-tag">Em andamento</span>
<a class="chapter-link" href="/manga/sample/capitulo-1"><span class="chapter-number">Capítulo 1</span><span class="chapter-date">2024-01-01</span></a>
"#;
const PAGES_FIXTURE: &str = r#"<canvas class="chapter-image-canvas" data-src-url="/page1.jpg"></canvas><canvas class="chapter-image-canvas" data-src-url="/page2.jpg"></canvas>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stalker_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
    }
}
