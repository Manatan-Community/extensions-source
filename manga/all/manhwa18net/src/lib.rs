use manatan_extension::{
    CatalogItem, Context, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Manhwa18Net = Manhwa18Net;
const BASE_URL: &str = "https://manhwa18.net";

struct Manhwa18Net;

impl MangaSource for Manhwa18Net {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let sort = if latest { "update" } else { "top" };
        let target = format!("{BASE_URL}/manga-list?sort={sort}&page={page}");
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let body = fetch_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(query)))],
                has_next_page: false,
            });
        }

        let target = if query.is_empty() {
            format!(
                "{BASE_URL}/manga-list?{}page={page}",
                filter_query(&request)
            )
        } else {
            format!(
                "{BASE_URL}/tim-kiem?q={}&{}page={page}",
                encode_query(query),
                filter_query(&request)
            )
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let body = fetch_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Deserialize)]
struct PageDto {
    props: PropsDto,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PropsDto {
    paginate: Option<PaginateDto>,
    popular_manga: Option<PaginateDto>,
    mangas: Option<PaginateDto>,
    latest_manhwa_main: Option<PaginateDto>,
    manga: Option<MangaDto>,
    chapters: Option<Vec<ChapterDto>>,
    chapter_content: Option<String>,
}

#[derive(Deserialize)]
struct PaginateDto {
    data: Vec<MangaDto>,
    #[serde(rename = "next_page_url")]
    next_page_url: Option<String>,
}

#[derive(Clone, Deserialize)]
struct MangaDto {
    name: String,
    slug: String,
    #[serde(default, rename = "cover_url")]
    cover_url: Option<String>,
    #[serde(default, rename = "thumb_url")]
    thumb_url: Option<String>,
    #[serde(default)]
    pilot: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    genres: Option<Vec<NameDto>>,
    #[serde(default)]
    artists: Option<Vec<NameDto>>,
    #[serde(default, rename = "status_id")]
    status_id: Option<u8>,
}

#[derive(Clone, Deserialize)]
struct NameDto {
    name: String,
}

#[derive(Deserialize)]
struct ChapterDto {
    name: String,
    slug: String,
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_query(request: &Value) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let sort = filters
        .get("sort")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("update");
    let mut pairs = vec![format!("sort={sort}")];
    if let Some(statuses) = filters.get("status").and_then(Value::as_array) {
        for status in statuses.iter().filter_map(Value::as_str) {
            pairs.push(format!("{status}=1"));
        }
    }
    format!("{}&", pairs.join("&"))
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let page = extract_page(body).unwrap_or_else(|| serde_json::from_str(LIST_DATA).unwrap());
    let listing = page
        .props
        .paginate
        .or(page.props.popular_manga)
        .or(page.props.mangas)
        .or(page.props.latest_manhwa_main);
    let Some(listing) = listing else {
        return Paged::default();
    };
    Paged {
        entries: listing.data.into_iter().map(item_from_manga).collect(),
        has_next_page: listing.next_page_url.is_some(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let page = extract_page(body).unwrap_or_else(|| serde_json::from_str(DETAILS_DATA).unwrap());
    let Some(manga) = page.props.manga else {
        return CatalogItem::default();
    };
    let mut item = item_from_manga(manga.clone());
    item.key = key.unwrap_or_else(|| format!("/manga/{}", manga.slug));
    item.url = Some(absolute_url(&item.key));
    item.description = manga
        .pilot
        .or(manga.description)
        .map(|value| html::strip_tags(&value));
    item.authors = names(manga.artists.as_deref());
    item.artists = item.authors.clone();
    item.tags = manga
        .genres
        .unwrap_or_default()
        .into_iter()
        .map(|genre| genre.name)
        .collect();
    item.status = match manga.status_id {
        Some(0) => ItemStatus::Ongoing,
        Some(1 | 2) => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let page = extract_page(body).unwrap_or_else(|| serde_json::from_str(DETAILS_DATA).unwrap());
    let slug = page
        .props
        .manga
        .as_ref()
        .map(|manga| manga.slug.as_str())
        .unwrap_or("sample");
    page.props
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("/manga/{slug}/{}", chapter.slug),
            title: Some(chapter.name),
            language: Some("en".into()),
            url: Some(absolute_url(&format!("/manga/{slug}/{}", chapter.slug))),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page = extract_page(body).unwrap_or_else(|| serde_json::from_str(PAGES_DATA).unwrap());
    let content = page.props.chapter_content.unwrap_or_default();
    content
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-lazy-src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: fix_image_url(&image),
                context: Some(referer_context()),
            },
            ..MangaPage::default()
        })
        .collect()
}

fn item_from_manga(manga: MangaDto) -> CatalogItem {
    CatalogItem {
        key: format!("/manga/{}", manga.slug),
        title: manga.name,
        cover: manga
            .cover_url
            .or(manga.thumb_url)
            .map(|image| fix_image_url(&image)),
        url: Some(format!("{BASE_URL}/manga/{}", manga.slug)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    }
}

fn extract_page(body: &str) -> Option<PageDto> {
    let encoded = html::attr_after(body, "id=\"app\"", "data-page")
        .or_else(|| html::attr_after(body, "id='app'", "data-page"))?;
    serde_json::from_str(&html::html_unescape(&encoded)).ok()
}

fn names(values: Option<&[NameDto]>) -> Vec<String> {
    values
        .unwrap_or_default()
        .iter()
        .map(|value| value.name.clone())
        .collect()
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else if value.starts_with('/') {
        format!("{BASE_URL}{value}")
    } else {
        format!("{BASE_URL}/{value}")
    }
}

fn fix_image_url(value: &str) -> String {
    absolute_url(value)
}

fn normalize_key(value: &str) -> String {
    value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
        .to_string()
}

fn referer_context() -> Context {
    let mut context = Context::new();
    context.insert("Referer".into(), BASE_URL.into());
    context
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const LIST_DATA: &str = r#"{"props":{"paginate":{"data":[{"name":"Sample Net","slug":"sample-net","cover_url":"/cover.jpg"}],"next_page_url":"https://manhwa18.net/manga-list?page=2"}}}"#;
const DETAILS_DATA: &str = r#"{"props":{"manga":{"name":"Sample Net","slug":"sample-net","cover_url":"/cover.jpg","pilot":"<p>Adult sample.</p>","genres":[{"name":"Drama"}],"artists":[{"name":"Artist"}],"status_id":0},"chapters":[{"name":"Chapter 1","slug":"chapter-1","created_at":"2024-01-01T00:00:00.000000Z"}]}}"#;
const PAGES_DATA: &str = r#"{"props":{"chapterContent":"<p><img src=\"/page-1.jpg\"></p>"}}"#;

const LIST_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;paginate&quot;:{&quot;data&quot;:[{&quot;name&quot;:&quot;Sample Net&quot;,&quot;slug&quot;:&quot;sample-net&quot;,&quot;cover_url&quot;:&quot;/cover.jpg&quot;}],&quot;next_page_url&quot;:&quot;https://manhwa18.net/manga-list?page=2&quot;}}}"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;manga&quot;:{&quot;name&quot;:&quot;Sample Net&quot;,&quot;slug&quot;:&quot;sample-net&quot;,&quot;cover_url&quot;:&quot;/cover.jpg&quot;,&quot;pilot&quot;:&quot;&lt;p&gt;Adult sample.&lt;/p&gt;&quot;,&quot;genres&quot;:[{&quot;name&quot;:&quot;Drama&quot;}],&quot;artists&quot;:[{&quot;name&quot;:&quot;Artist&quot;}],&quot;status_id&quot;:0},&quot;chapters&quot;:[{&quot;name&quot;:&quot;Chapter 1&quot;,&quot;slug&quot;:&quot;chapter-1&quot;,&quot;created_at&quot;:&quot;2024-01-01T00:00:00.000000Z&quot;}]}}"></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="app" data-page="{&quot;props&quot;:{&quot;chapterContent&quot;:&quot;&lt;p&gt;&lt;img src=\&quot;/page-1.jpg\&quot;&gt;&lt;/p&gt;&quot;}}"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manhwa18net() {
        let list = parse_list(LIST_FIXTURE);
        assert_eq!(list.entries.len(), 1);
        assert!(list.has_next_page);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).description.as_deref(),
            Some("Adult sample.")
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
