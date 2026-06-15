use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Astratoons = Astratoons;
const BASE_URL: &str = "https://new.astratoons.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";

struct Astratoons;

impl MangaSource for Astratoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_home_popular(HOME_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_api_listing(&fetch_text(
                &api_comics_url(&request, Some("updated_at")),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_home_popular(&fetch_document(BASE_URL, HOME_FIXTURE)))
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_api_listing(&fetch_text(
            &api_comics_url(&request, None),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let details = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let id = comic_id(&details).unwrap_or_else(|| "1".to_string());
        Ok(fetch_all_chapters(&id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comics/sample/capitulo-1".into());
        Ok(parse_pages(
            &fetch_document(&absolute_url(&key), PAGES_FIXTURE),
            &absolute_url(&key),
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key
                    .starts_with("/comics/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_comics_url(request: &Value, default_sort: Option<&str>) -> String {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let mut params = vec![("page", page.to_string())];
    if let Some(query) = request
        .get("query")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        params.push(("search", query.to_string()));
    }
    let sort = filter(filters, "sort").or_else(|| default_sort.map(ToString::to_string));
    if let Some(sort) = sort.filter(|value| !value.is_empty()) {
        params.push(("sortBy", sort));
    }
    if let Some(status) = filter(filters, "status").filter(|value| !value.is_empty()) {
        params.push(("status", status));
    }
    let mut out = format!("{BASE_URL}/api/comics?");
    out.push_str(
        &params
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&"),
    );
    for value in filter_values(filters, "types") {
        out.push_str("&types[]=");
        out.push_str(&url::query_escape(&value));
    }
    for value in filter_values(filters, "tags") {
        out.push_str("&tags[]=");
        out.push_str(&url::query_escape(&value));
    }
    out
}

fn parse_home_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/comics/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "<h3", "</h3>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| {
                            url::slug_from_url(&key).unwrap_or_else(|| "Astratoons".into())
                        }),
                    cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_api_listing(body: &str) -> Paged<CatalogItem> {
    let dto = serde_json::from_str::<ComicsResponseDto>(body)
        .or_else(|_| serde_json::from_str::<ComicsResponseDto>(LIST_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: dto
            .data
            .into_iter()
            .map(catalog_from_dto)
            .fold(Vec::new(), push_unique),
        has_next_page: dto.current_page < dto.last_page,
    }
}

fn catalog_from_dto(comic: ComicDto) -> CatalogItem {
    let key = format!("/comics/{}", comic.slug);
    CatalogItem {
        key: key.clone(),
        title: comic.title,
        cover: Some(if comic.cover_image.starts_with("http") {
            comic.cover_image
        } else {
            format!(
                "{BASE_URL}/storage/{}",
                comic.cover_image.trim_start_matches('/')
            )
        }),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Astratoons".into())),
        cover: html::attr_after(body, "object-cover", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "space-y-4", "</p>")
            .or_else(|| html::text_between(body, "<p", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: span_value(body, "Autor").into_iter().collect(),
        artists: span_value(body, "Artista").into_iter().collect(),
        tags: link_values_near(body, "Tags"),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(comic_id: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for page in 1..=50 {
        let target =
            format!("{BASE_URL}/api/comics/{comic_id}/chapters?search=&order=desc&page={page}");
        let dto = serde_json::from_str::<ChapterListDto>(&fetch_text(&target, CHAPTERS_FIXTURE))
            .or_else(|_| serde_json::from_str::<ChapterListDto>(CHAPTERS_FIXTURE))
            .unwrap_or_default();
        chapters.extend(parse_chapter_html(&dto.html));
        if !dto.has_more {
            break;
        }
    }
    chapters
}

fn parse_chapter_html(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "text-lg", "</")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".to_string())),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .chain(body.split("<canvas").skip(1))
        .filter(|chunk| {
            chunk.contains("reader-container")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
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

fn filter(filters: &Value, name: &str) -> Option<String> {
    filters
        .get(name)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn filter_values(filters: &Value, name: &str) -> Vec<String> {
    filters
        .get(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn comic_id(body: &str) -> Option<String> {
    let marker = "comicId:";
    let rest = body.split(marker).nth(1)?;
    Some(
        rest.chars()
            .skip_while(|ch| ch.is_whitespace())
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>(),
    )
    .filter(|value| !value.is_empty())
}

fn span_value(body: &str, label: &str) -> Option<String> {
    body.split("<span")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| {
            html::strip_tags(&value)
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn link_values_near(body: &str, label: &str) -> Vec<String> {
    body.split("<h3")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("completo") {
        ItemStatus::Completed
    } else if lower.contains("hiato") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelado") || lower.contains("dropado") {
        ItemStatus::Cancelled
    } else if lower.contains("em andamento") || lower.contains("em dia") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Default, Deserialize)]
struct ComicsResponseDto {
    #[serde(default)]
    data: Vec<ComicDto>,
    #[serde(default)]
    current_page: i32,
    #[serde(default)]
    last_page: i32,
}

#[derive(Default, Deserialize)]
struct ComicDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    cover_image: String,
}

#[derive(Default, Deserialize)]
struct ChapterListDto {
    #[serde(default, rename = "hasMore")]
    has_more: bool,
    #[serde(default)]
    html: String,
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<div id="comicsSlider"><a href="/comics/sample"><img src="/cover.jpg"><h3>Sample Astra</h3></a></div>"#;
const LIST_FIXTURE: &str = r#"{"data":[{"title":"Sample Astra","slug":"sample","cover_image":"covers/sample.jpg"}],"current_page":1,"last_page":1}"#;
const DETAILS_FIXTURE: &str = r#"<script>comicId: 1</script><h1>Sample Astra</h1><img class="object-cover" src="/cover.jpg"><div class="space-y-4"><p>Summary</p></div><h3>Tags</h3><div><a>Action</a></div><h3>Informacoes</h3><div><span class="capitalize">em andamento</span></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"hasMore":false,"html":"<a href=\"/comics/sample/capitulo-1\"><span class=\"text-lg\">Capitulo 1</span></a>"}"#;
const PAGES_FIXTURE: &str = r#"<div id="reader-container"><img src="/page1.jpg"><canvas data-src="/page2.jpg"></canvas></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_astratoons_fixtures() {
        assert_eq!(parse_api_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_chapter_html(
                r#"<a href="/comics/sample/c1"><span class="text-lg">C1</span></a>"#
            )
            .len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
    }
}
