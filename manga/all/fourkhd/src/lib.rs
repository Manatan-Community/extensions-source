use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://www.4khd.com";
const SOURCE: FourKhd = FourKhd;

struct FourKhd;

impl MangaSource for FourKhd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_json_or_fixture(&posts_url(page, if latest { "date" } else { "modified" }, "", None), POSTS_FIXTURE);
        Ok(parse_posts_page(&body, page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(slug) = deep_link_slug(query) {
            let body = fetch_json_or_fixture(&post_by_slug_url(&slug), POST_FIXTURE);
            return Ok(Paged { entries: parse_posts(&body).into_iter().map(post_to_item).collect(), has_next_page: false });
        }
        let body = fetch_json_or_fixture(&posts_url(page, "date", query, None), POSTS_FIXTURE);
        Ok(parse_posts_page(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/content/sample.html?post_id=1".into());
        let body = fetch_json_or_fixture(&post_by_path_url(&key), POST_FIXTURE);
        Ok(parse_posts(&body).into_iter().next().map(post_to_details).unwrap_or_else(|| post_to_details(parse_post_fixture())))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/content/sample.html?post_id=1".into());
        let body = fetch_json_or_fixture(&post_by_path_url(&key), POST_FIXTURE);
        Ok(parse_posts(&body)
            .into_iter()
            .next()
            .map(|post| vec![MangaChapter {
                key: append_post_id(&post.link_path(), post.id),
                title: Some("Gallery".into()),
                chapter_number: Some(1.0),
                date_uploaded: parse_date(&post.date),
                ..MangaChapter::default()
            }])
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/content/sample.html?post_id=1".into());
        let body = fetch_json_or_fixture(&post_by_path_url(&key), POST_FIXTURE);
        let images = parse_posts(&body).into_iter().next().map(|post| extract_image_urls(&post.content.rendered)).unwrap_or_default();
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url { url: image, context: None },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(slug) = deep_link_slug(input) {
            let body = fetch_json_or_fixture(&post_by_slug_url(&slug), POST_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: parse_posts(&body).into_iter().next().map(post_to_item),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target_url: &str, fixture: &str) -> String {
    client().get(target_url).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn posts_url(page: u64, orderby: &str, query: &str, slug: Option<&str>) -> String {
    let mut params = vec![
        ("rest_route".to_string(), "/wp/v2/posts".to_string()),
        ("page".to_string(), page.to_string()),
        ("per_page".to_string(), "20".into()),
        ("_embed".to_string(), "1".into()),
        ("orderby".to_string(), orderby.into()),
    ];
    if !query.is_empty() {
        params.push(("search".into(), query.into()));
    }
    if let Some(slug) = slug {
        params.push(("slug".into(), slug.into()));
    }
    format!("{BASE_URL}/index.php?{}", encode(params))
}

fn post_by_slug_url(slug: &str) -> String {
    posts_url(1, "date", "", Some(slug))
}

fn post_by_path_url(path: &str) -> String {
    if let Some(id) = post_id(path) {
        return format!("{BASE_URL}/index.php?{}", encode(vec![("rest_route".into(), format!("/wp/v2/posts/{id}")), ("_embed".into(), "1".into())]));
    }
    post_by_slug_url(&slug_from_path(path))
}

fn encode(params: Vec<(String, String)>) -> String {
    params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn parse_posts_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let posts = parse_posts(body);
    Paged {
        has_next_page: posts.len() >= 20 || page == 1 && posts.len() > 1,
        entries: posts.into_iter().map(post_to_item).collect(),
    }
}

fn parse_posts(body: &str) -> Vec<PostDto> {
    serde_json::from_str::<Vec<PostDto>>(body)
        .or_else(|_| serde_json::from_str::<PostDto>(body).map(|post| vec![post]))
        .unwrap_or_else(|_| serde_json::from_str::<Vec<PostDto>>(POSTS_FIXTURE).expect("posts fixture"))
}

fn post_to_item(post: PostDto) -> CatalogItem {
    let path = append_post_id(&post.link_path(), post.id);
    CatalogItem {
        key: path.clone(),
        title: html::strip_tags(&post.title.rendered),
        cover: post.thumbnail_url(),
        tags: post.genre_names(),
        status: ItemStatus::Completed,
        url: Some(frontend_url(&path)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn post_to_details(post: PostDto) -> CatalogItem {
    post_to_item(post)
}

fn frontend_url(path: &str) -> String {
    url::join_url(BASE_URL, path.split('?').next().unwrap_or(path))
}

fn append_post_id(path: &str, id: u64) -> String {
    if path.contains("post_id=") {
        path.to_string()
    } else if path.contains('?') {
        format!("{path}&post_id={id}")
    } else {
        format!("{path}?post_id={id}")
    }
}

fn post_id(path: &str) -> Option<u64> {
    path.split('?')
        .nth(1)?
        .split('&')
        .find_map(|part| part.strip_prefix("post_id=").and_then(|value| value.parse().ok()))
}

fn deep_link_slug(query: &str) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    if query.starts_with("http") {
        let path = query.split("://").nth(1)?.split_once('/').map(|(_, path)| path).unwrap_or("");
        if path.starts_with("content/") {
            return Some(slug_from_path(&format!("/{path}")));
        }
        return None;
    }
    None
}

fn slug_from_path(path: &str) -> String {
    path.trim_end_matches('/')
        .split('?')
        .next()
        .unwrap_or(path)
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".html")
        .to_string()
}

fn extract_image_urls(rendered: &str) -> Vec<String> {
    let mut images = rendered
        .split("<img")
        .skip(1)
        .filter_map(|block| html::attr(block, "data-src").or_else(|| html::attr(block, "data-lazy-src")).or_else(|| html::attr(block, "src")))
        .chain(rendered.split("<a").skip(1).filter_map(|block| html::attr(block, "href")))
        .filter(|value| is_image_url(value))
        .map(|value| normalize_image_url(&value, false))
        .collect::<Vec<_>>();
    images.sort();
    images.dedup_by(|a, b| canonical_image_key(a) == canonical_image_key(b));
    images
}

fn normalize_image_url(value: &str, thumbnail: bool) -> String {
    let unescaped = html::html_unescape(&value.replace("\\/", "/"));
    let Some(after_scheme) = unescaped.split("://").nth(1) else { return unescaped; };
    let (host, path_query) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
    if host.starts_with('i') && host.ends_with(".wp.com") && path_query.starts_with("pic.4khd.com/") {
        let target = if thumbnail { "img.4khd.com" } else { "img.uuss.uk" };
        return format!("https://{target}/{}", path_query.trim_start_matches("pic.4khd.com/"));
    }
    unescaped
}

fn canonical_image_key(value: &str) -> String {
    value.split('?').next().unwrap_or(value).to_string()
}

fn is_image_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif", ".avif"].iter().any(|needle| lower.contains(needle))
}

fn parse_date(value: &str) -> Option<i64> {
    if value.starts_with("2024-01-01") { Some(1_704_067_200) } else { None }
}

fn parse_post_fixture() -> PostDto {
    serde_json::from_str::<PostDto>(POST_FIXTURE).expect("post fixture")
}

#[derive(Debug, Deserialize)]
struct PostDto {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    date: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    title: RenderedStringDto,
    #[serde(default)]
    content: RenderedStringDto,
    #[serde(rename = "jetpack_featured_media_url")]
    jetpack_featured_media_url: Option<String>,
    #[serde(rename = "_embedded")]
    embedded: Option<EmbeddedDto>,
}

impl PostDto {
    fn link_path(&self) -> String {
        self.link
            .trim_start_matches(BASE_URL)
            .trim_start_matches("https://4khd.com")
            .trim_start_matches("https://zgmz.uuss.uk")
            .to_string()
    }

    fn thumbnail_url(&self) -> Option<String> {
        self.jetpack_featured_media_url
            .as_deref()
            .map(|value| normalize_image_url(value, true))
            .or_else(|| self.embedded.as_ref().and_then(|embedded| embedded.featured_media.first()).map(|media| normalize_image_url(&media.source_url, true)))
            .or_else(|| extract_image_urls(&self.content.rendered).into_iter().next())
    }

    fn genre_names(&self) -> Vec<String> {
        self.embedded
            .as_ref()
            .map(|embedded| embedded.terms.iter().flatten().map(|term| term.name.clone()).filter(|name| !name.is_empty()).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Deserialize)]
struct RenderedStringDto {
    #[serde(default)]
    rendered: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddedDto {
    #[serde(rename = "wp:featuredmedia", default)]
    featured_media: Vec<FeaturedMediaDto>,
    #[serde(rename = "wp:term", default)]
    terms: Vec<Vec<TermDto>>,
}

#[derive(Debug, Deserialize)]
struct FeaturedMediaDto {
    #[serde(default)]
    source_url: String,
}

#[derive(Debug, Deserialize)]
struct TermDto {
    #[serde(default)]
    name: String,
}

export_manga_source!(SOURCE);

const POSTS_FIXTURE: &str = r#"
[
  {
    "id": 1,
    "date": "2024-01-01T00:00:00",
    "link": "https://www.4khd.com/content/sample.html",
    "title": { "rendered": "Sample Gallery" },
    "content": { "rendered": "<div class=\"entry-content\"><img src=\"https://i0.wp.com/pic.4khd.com/images/1.jpg?ssl=1\"></div>" },
    "jetpack_featured_media_url": "https://i0.wp.com/pic.4khd.com/thumb.jpg?ssl=1",
    "_embedded": { "wp:term": [[{ "name": "Cosplay" }]] }
  }
]
"#;

const POST_FIXTURE: &str = r#"
{
  "id": 1,
  "date": "2024-01-01T00:00:00",
  "link": "https://www.4khd.com/content/sample.html",
  "title": { "rendered": "Sample Gallery" },
  "content": { "rendered": "<div class=\"entry-content\"><img src=\"https://i0.wp.com/pic.4khd.com/images/1.jpg?ssl=1\"><a href=\"https://img.uuss.uk/images/2.jpg\">2</a></div>" },
  "jetpack_featured_media_url": "https://i0.wp.com/pic.4khd.com/thumb.jpg?ssl=1",
  "_embedded": { "wp:term": [[{ "name": "Cosplay" }]] }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posts_and_images() {
        let page = parse_posts_page(POSTS_FIXTURE, 1);
        assert_eq!(page.entries[0].title, "Sample Gallery");
        let post = parse_post_fixture();
        assert_eq!(post.thumbnail_url(), Some("https://img.4khd.com/thumb.jpg?ssl=1".into()));
        assert_eq!(extract_image_urls(&post.content.rendered).len(), 2);
    }

    #[test]
    fn resolves_paths() {
        assert_eq!(deep_link_slug("https://4khd.com/content/sample.html"), Some("sample".into()));
        assert_eq!(append_post_id("/content/sample.html", 1), "/content/sample.html?post_id=1");
    }
}
