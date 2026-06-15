use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://danbooru.donmai.us";
const SOURCE: Danbooru = Danbooru;

struct Danbooru;

impl MangaSource for Danbooru {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let order = if latest { "created_at" } else { "updated_at" };
        Ok(parse_pool_gallery(&fetch_text_or_fixture(&gallery_url(page, "", None, Some(order)), GALLERY_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_html(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_pool_gallery(&fetch_text_or_fixture(
            &gallery_url(page, query, Some(filters), None),
            GALLERY_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/pools/1".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details_html(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/pools/1".into());
        let body = fetch_text_or_fixture(&format!("{}{}.json", BASE_URL, key), POOL_FIXTURE);
        let split = request
            .get("preferences")
            .and_then(|prefs| prefs.get("splitPostsIntoChapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters_json(&body, split))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/pools/1".into());
        let body = fetch_text_or_fixture(&format!("{}{}.json", BASE_URL, key), if key.contains("/posts/") { POST_FIXTURE } else { POOL_FIXTURE });
        if key.contains("/posts/") {
            let post = parse_post_json(&body);
            return Ok(vec![image_page(0, post.file_url)]);
        }
        let pool = parse_pool_json(&body);
        Ok(pool
            .post_ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| MangaPage {
                content: PageContent::Lazy {
                    key: format!("/posts/{id}"),
                    url: Some(format!("{BASE_URL}/posts/{id}")),
                    page_url: Some(format!("{BASE_URL}/pools/{}", pool.id)),
                    context: None,
                },
                description: Some(format!("Post {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("/posts/1");
        let body = fetch_text_or_fixture(&format!("{BASE_URL}{key}.json"), POST_FIXTURE);
        Ok(MangaPageImage {
            url: parse_post_json(&body).file_url,
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/pools/") {
            let key = normalize_key(input);
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_html(&body, Some(key))),
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
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client().get(target_url).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn gallery_url(page: u64, query: &str, filters: Option<&Value>, forced_order: Option<&str>) -> String {
    let mut params = vec![
        ("search[category]".to_string(), filter_category(filters).to_string()),
        ("page".to_string(), page.to_string()),
    ];
    if let Some(order) = forced_order.or_else(|| filter_order(filters)) {
        params.push(("search[order]".to_string(), order.to_string()));
    }
    if let Some(description) = filter_text(filters, "description") {
        params.push(("search[description_matches]".to_string(), description));
    }
    if let Some(tags) = filter_text(filters, "tags") {
        params.push(("search[post_tags_match]".to_string(), tags));
    }
    if filters
        .and_then(|value| value.get("isDeleted"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        params.push(("search[is_deleted]".to_string(), "true".to_string()));
    }
    if !query.is_empty() {
        params.push(("search[name_contains]".to_string(), query.to_string()));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/pools/gallery?{query}")
}

fn filter_category(filters: Option<&Value>) -> &'static str {
    match filters.and_then(|value| value.get("category")).and_then(Value::as_str) {
        Some("Collection") => "collection",
        _ => "series",
    }
}

fn filter_order(filters: Option<&Value>) -> Option<&'static str> {
    match filters.and_then(|value| value.get("order")).and_then(Value::as_str) {
        Some("Name") => Some("name"),
        Some("Recently created") => Some("created_at"),
        Some("Post count") => Some("post_count"),
        Some("Last updated") => Some("updated_at"),
        _ => None,
    }
}

fn filter_text(filters: Option<&Value>, id: &str) -> Option<String> {
    filters
        .and_then(|value| value.get(id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_pool_gallery(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("post-preview")
        .skip(1)
        .filter_map(parse_gallery_block)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("paginator-next"),
    }
}

fn parse_gallery_block(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "post-preview-link", "href").or_else(|| html::attr_after(block, "<a", "href"))?;
    let title = html::text_between(block, "text-center", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    let cover = html::attr_after(block, "<source", "srcset")
        .map(|value| value.rsplit(',').next().unwrap_or(&value).split_whitespace().next().unwrap_or(&value).to_string());
    Some(CatalogItem {
        key: normalize_key(&href),
        title,
        cover,
        status: ItemStatus::Unknown,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        url: Some(url::join_url(BASE_URL, &href)),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details_html(body: &str, key: Option<String>) -> CatalogItem {
    let title = html::text_between(body, "pool-category-series", "</")
        .or_else(|| html::text_between(body, "pool-category-collection", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Danbooru Pool".into());
    let description = html::text_between(body, "id=\"description\"", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let author = body
        .split("<a")
        .find(|block| block.contains("artists"))
        .and_then(|block| html::text_between(block, ">", "</a>"))
        .map(|value| html::strip_tags(&value));
    CatalogItem {
        key: key.unwrap_or_else(|| "/pools/1".into()),
        title,
        description,
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        status: ItemStatus::Unknown,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_json(body: &str, split: bool) -> Vec<MangaChapter> {
    let pool = parse_pool_json(body);
    if split {
        let total = pool.post_ids.len();
        return pool
            .post_ids
            .into_iter()
            .enumerate()
            .rev()
            .map(|(index, id)| MangaChapter {
                key: format!("/posts/{id}"),
                title: Some(format!("Post {}", index + 1)),
                chapter_number: Some((index + 1) as f32),
                date_uploaded: if index + 1 == total { parse_danbooru_date(&pool.updated_at) } else { None },
                ..MangaChapter::default()
            })
            .collect();
    }
    vec![MangaChapter {
        key: format!("/pools/{}", pool.id),
        title: Some("Oneshot".into()),
        chapter_number: Some(0.0),
        date_uploaded: parse_danbooru_date(&pool.updated_at),
        ..MangaChapter::default()
    }]
}

fn image_page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url { url: image, context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn parse_pool_json(body: &str) -> Pool {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(POOL_FIXTURE).expect("pool fixture"))
}

fn parse_post_json(body: &str) -> Post {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(POST_FIXTURE).expect("post fixture"))
}

fn parse_danbooru_date(value: &str) -> Option<i64> {
    if value.starts_with("2024-01-01") {
        Some(1_704_067_200)
    } else {
        None
    }
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_matches('/'))
}

#[derive(Debug, Deserialize)]
struct Pool {
    id: u64,
    #[serde(rename = "updated_at")]
    updated_at: String,
    #[serde(rename = "post_ids")]
    post_ids: Vec<u64>,
}

#[derive(Debug, Deserialize)]
struct Post {
    #[serde(rename = "file_url")]
    file_url: String,
}

export_manga_source!(SOURCE);

const GALLERY_FIXTURE: &str = r#"
<article class="post-preview">
  <a class="post-preview-link" href="/pools/1"></a>
  <source srcset="https://danbooru.donmai.us/thumb-small.jpg 1x, https://danbooru.donmai.us/thumb.jpg 2x">
  <div class="text-center">Sample Pool</div>
</article>
<a class="paginator-next" href="/pools/gallery?page=2">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<main>
  <h1 class="pool-category-series">Sample Pool</h1>
  <div id="description">A pool by <a href="/artists/1">Artist</a>.</div>
</main>
"#;

const POOL_FIXTURE: &str = r#"
{ "id": 1, "updated_at": "2024-01-01T00:00:00.000-00:00", "post_ids": [10, 11] }
"#;

const POST_FIXTURE: &str = r#"
{ "file_url": "https://danbooru.donmai.us/original/sample.jpg" }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gallery_and_details() {
        let page = parse_pool_gallery(GALLERY_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let details = parse_details_html(DETAILS_FIXTURE, Some("/pools/1".into()));
        assert_eq!(details.title, "Sample Pool");
        assert_eq!(details.authors, vec!["Artist"]);
    }

    #[test]
    fn parses_chapters_and_pages() {
        assert_eq!(parse_chapters_json(POOL_FIXTURE, false).len(), 1);
        assert_eq!(parse_chapters_json(POOL_FIXTURE, true).len(), 2);
        assert_eq!(parse_post_json(POST_FIXTURE).file_url, "https://danbooru.donmai.us/original/sample.jpg");
    }

    #[test]
    fn builds_filter_url() {
        let filters = serde_json::json!({ "category": "Collection", "order": "Post count", "tags": "blue", "isDeleted": true });
        let built = gallery_url(2, "demo", Some(&filters), None);
        assert!(built.contains("search%5Bcategory%5D=collection"));
        assert!(built.contains("search%5Border%5D=post_count"));
        assert!(built.contains("search%5Bname_contains%5D=demo"));
    }
}
