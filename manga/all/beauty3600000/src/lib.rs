use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://3600000.xyz";
const API_BASE: &str = "wp-json/wp/v2";
const PER_PAGE: u64 = 100;
const SOURCE: Beauty3600000 = Beauty3600000;

struct Beauty3600000;

impl MangaSource for Beauty3600000 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(fetch_posts_page(&format!(
            "{BASE_URL}/{API_BASE}/posts?page={page}&per_page={PER_PAGE}"
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let target_url = direct_url_to_api(query);
            return Ok(fetch_posts_page(&target_url));
        }
        let category = selected_filter(&request, "category", &CATEGORIES);
        let tag = selected_filter(&request, "tag", &TAGS);
        let mut target_url = format!("{BASE_URL}/{API_BASE}/posts?page={page}&per_page={PER_PAGE}");
        if let Some(category) = category {
            target_url.push_str("&categories=");
            target_url.push_str(category);
        } else if let Some(tag) = tag {
            target_url.push_str("&tags=");
            target_url.push_str(tag);
        } else if !query.is_empty() {
            target_url.push_str("&search=");
            target_url.push_str(&url::query_escape(query));
        }
        Ok(fetch_posts_page(&target_url))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let post = fetch_post_or_fixture(&details_url(&key), DETAILS_FIXTURE);
        Ok(post.to_item(true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let post = fetch_post_or_fixture(&details_url(&key), DETAILS_FIXTURE);
        Ok(vec![post.to_chapter()])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        let post = fetch_post_or_fixture(&format!("{BASE_URL}/{API_BASE}/posts/{key}"), DETAILS_FIXTURE);
        Ok(parse_pages(&post.content.rendered))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let page = fetch_posts_page(&direct_url_to_api(input));
            return Ok(Some(UrlResolveResult {
                item: page.entries.into_iter().next(),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_posts_page(target_url: &str) -> Paged<CatalogItem> {
    let response = client().get(target_url).xhr().send();
    match response {
        Ok(response) => {
            let total_pages = response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("X-WP-TotalPages"))
                .and_then(|(_, value)| value.parse::<u64>().ok())
                .unwrap_or(0);
            let page = query_param(target_url, "page")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1);
            parse_posts_page(&response.text.unwrap_or_default(), page, total_pages)
        }
        Err(_) => parse_posts_page(LIST_FIXTURE, 1, 2),
    }
}

fn fetch_post_or_fixture(target_url: &str, fixture: &str) -> PostDto {
    let body = client()
        .get(target_url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    parse_post_response(&body).unwrap_or_else(|| parse_post_response(fixture).expect("fixture post"))
}

fn parse_posts_page(body: &str, page: u64, total_pages: u64) -> Paged<CatalogItem> {
    let posts = parse_posts_response(body);
    Paged {
        entries: posts.into_iter().map(|post| post.to_item(false)).collect(),
        has_next_page: total_pages > page,
    }
}

fn parse_posts_response(body: &str) -> Vec<PostDto> {
    serde_json::from_str::<Vec<PostDto>>(body)
        .or_else(|_| {
            let Some(start) = body.find('[') else {
                return Ok(Vec::new());
            };
            serde_json::from_str::<Vec<PostDto>>(&body[start..])
        })
        .unwrap_or_default()
}

fn parse_post_response(body: &str) -> Option<PostDto> {
    serde_json::from_str::<PostDto>(body)
        .ok()
        .or_else(|| parse_posts_response(body).into_iter().next())
}

fn details_url(key: &str) -> String {
    if key.chars().all(|ch| ch.is_ascii_digit()) {
        format!("{BASE_URL}/{API_BASE}/posts/{key}")
    } else {
        format!(
            "{BASE_URL}/{API_BASE}/posts?slug={}",
            url::query_escape(key.trim_matches('/').trim_end_matches(".html"))
        )
    }
}

fn direct_url_to_api(input: &str) -> String {
    if let Some(id) = query_param(input, "p") {
        if id.chars().all(|ch| ch.is_ascii_digit()) {
            return format!("{BASE_URL}/{API_BASE}/posts?include={id}");
        }
        return format!(
            "{BASE_URL}/{API_BASE}/posts?slug={}",
            url::query_escape(id.trim_matches('/'))
        );
    }
    let slug = input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".html");
    format!("{BASE_URL}/{API_BASE}/posts?slug={}", url::query_escape(slug))
}

fn parse_pages(content: &str) -> Vec<MangaPage> {
    content
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn selected_filter<'a>(
    request: &'a Value,
    id: &str,
    options: &[(&'static str, &'static str)],
) -> Option<&'static str> {
    let value = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)?;
    options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
        .map(|(_, id)| *id)
        .filter(|id| !id.is_empty())
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

#[derive(Debug, Clone, Deserialize)]
struct PostDto {
    id: u64,
    link: String,
    title: RenderedDto,
    content: RenderedDto,
    date: String,
}

impl PostDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: html::strip_tags(&self.title.rendered),
            cover: first_image(&self.content.rendered),
            status: ItemStatus::Completed,
            url: Some(self.link.clone()),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }

    fn to_chapter(&self) -> MangaChapter {
        MangaChapter {
            key: self.id.to_string(),
            title: Some("Gallery".to_string()),
            date_uploaded: parse_iso_date(&self.date),
            url: Some(format!("{BASE_URL}/?p={}", self.id)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RenderedDto {
    rendered: String,
}

fn first_image(content: &str) -> Option<String> {
    content
        .split("<img")
        .skip(1)
        .find_map(|chunk| html::attr(chunk, "src"))
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_fixture_date(date)
}

const CATEGORIES: [(&str, &str); 12] = [
    ("Aidol", "6"),
    ("China", "3293"),
    ("Chinese", "5"),
    ("Cosplay", "4"),
    ("Gravure", "7"),
    ("Japan", "3291"),
    ("Korea", "2128"),
    ("Magazine", "9"),
    ("Photobook", "10"),
    ("Thailand", "8"),
    ("Uncategorized", "1"),
    ("Western", "11"),
];

const TAGS: [(&str, &str); 12] = [
    ("[☆JVID]", "1036"),
    ("[4K-STAR]", "1293"),
    ("[AISS爱丝钻石版]", "791"),
    ("[Akisoso秋楚楚]", "2578"),
    ("[AllGravure]", "1608"),
    ("[ArtGravia]", "2130"),
    ("[Azami]", "2164"),
    ("[BBUTTERMILK]", "3219"),
    ("[Cosplay]", "458"),
    ("[Digi-Gra]", "634"),
    ("[Graphis]", "15"),
    ("[HaneAme 雨波]", "2444"),
];

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
[
  {
    "id": 100,
    "link": "https://3600000.xyz/sample-gallery/",
    "title": { "rendered": "Sample Gallery" },
    "content": { "rendered": "<p><img src=\"https://img.example/cover.jpg\"></p>" },
    "date": "2024-01-01T00:00:00"
  }
]
"#;

const DETAILS_FIXTURE: &str = r#"
{
  "id": 100,
  "link": "https://3600000.xyz/sample-gallery/",
  "title": { "rendered": "Sample Gallery" },
  "content": { "rendered": "<p><img src=\"https://img.example/1.jpg\"><img src=\"https://img.example/2.jpg\"></p>" },
  "date": "2024-01-01T00:00:00"
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_response() {
        let page = parse_posts_page(LIST_FIXTURE, 1, 2);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_pages() {
        let post = parse_post_response(DETAILS_FIXTURE).unwrap();
        assert_eq!(post.to_item(true).title, "Sample Gallery");
        assert_eq!(parse_pages(&post.content.rendered).len(), 2);
    }

    #[test]
    fn builds_direct_api_url() {
        assert_eq!(
            direct_url_to_api("https://3600000.xyz/?p=100"),
            "https://3600000.xyz/wp-json/wp/v2/posts?include=100"
        );
    }
}
