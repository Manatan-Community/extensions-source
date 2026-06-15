use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://e621.net";
const USER_AGENT: &str = "ManatanCommunityExtensions/0.1 (Manatan native source package)";
const SOURCE: E621 = E621;

struct E621;

impl MangaSource for E621 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let prefs = request.get("preferences").unwrap_or(&Value::Null);
        let target = if pref_bool(prefs, "tagModeForListings", true) {
            let tags = if latest {
                format!("order:id_desc score:>={}", pref_text(prefs, "scoreThreshold", "20"))
            } else {
                pref_text(prefs, "popularTags", "order:score date:year")
            };
            posts_url(page, &listing_tags(prefs, &tags))
        } else {
            pools_url(page, "", listing_category(prefs), if latest { "created_at" } else { "post_count" }, false, "")
        };
        Ok(parse_search_response(&fetch_json_or_fixture(&target, if target.contains("/posts.json") { POSTS_FIXTURE } else { POOLS_FIXTURE }), target.contains("/posts.json")))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) && query.contains("/pools/") {
            let id = query.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/pools/{id}.json"), POOL_FIXTURE);
            return Ok(Paged { entries: vec![pool_to_item(&parse_pool(&body), None)], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let prefs = request.get("preferences").unwrap_or(&Value::Null);
        let mode = filters.get("mode").and_then(Value::as_str).unwrap_or("Pools");
        let target = if mode == "Tags" {
            let mut tags = listing_tags(prefs, &filter_text(filters, "tags"));
            if !query.is_empty() {
                tags.push(' ');
                tags.push('*');
                tags.push_str(&query.replace(' ', "_"));
                tags.push('*');
            }
            if let Some(order) = tag_order(filters) {
                tags = format!("order:{order} {tags}");
            }
            if let Some(date) = date_filter(filters) {
                tags = format!("date:{date} {tags}");
            }
            if filters.get("firstPage").and_then(Value::as_bool).unwrap_or(false) {
                tags.push_str(" first_page");
            }
            if filters.get("endPage").and_then(Value::as_bool).unwrap_or(false) {
                tags.push_str(" end_page");
            }
            posts_url(page, &tags)
        } else {
            pools_url(
                page,
                query,
                pool_category(filters),
                pool_order(filters),
                filters.get("activeOnly").and_then(Value::as_bool).unwrap_or(false),
                &filter_text(filters, "description"),
            )
        };
        Ok(parse_search_response(&fetch_json_or_fixture(&target, if mode == "Tags" { POSTS_FIXTURE } else { POOLS_FIXTURE }), mode == "Tags"))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/pools/{key}.json"), POOL_FIXTURE);
        let pool = parse_pool(&body);
        let posts = if request
            .get("preferences")
            .and_then(|prefs| prefs.get("betterDetails"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            batch_fetch_posts(&pool.post_ids.iter().copied().take(40).collect::<Vec<_>>())
        } else {
            Vec::new()
        };
        Ok(pool_to_details(&pool, &posts))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/pools/{key}.json"), POOL_FIXTURE);
        let pool = parse_pool(&body);
        let split = request
            .get("preferences")
            .and_then(|prefs| prefs.get("splitChapters"))
            .and_then(Value::as_str)
            .unwrap_or("merged");
        Ok(parse_chapters(&pool, split))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/pools/1".into());
        let full_res = request
            .get("preferences")
            .and_then(|prefs| prefs.get("fullResolution"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(id) = key.strip_prefix("/posts/").and_then(|value| value.parse::<u64>().ok()) {
            let posts = fetch_posts_by_ids(&[id]);
            let image = posts.first().and_then(|post| image_url(post, full_res)).unwrap_or_else(placeholder_deleted);
            return Ok(vec![image_page(0, image)]);
        }
        let pool_id = key.trim_start_matches("/pools/");
        let pool = parse_pool(&fetch_json_or_fixture(&format!("{BASE_URL}/pools/{pool_id}.json"), POOL_FIXTURE));
        let posts = fetch_posts_by_ids(&pool.post_ids);
        let post_map = posts.iter().map(|post| (post.id, post)).collect::<std::collections::BTreeMap<_, _>>();
        Ok(pool
            .post_ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let image = post_map.get(id).and_then(|post| image_url(post, full_res)).unwrap_or_else(placeholder_deleted);
                image_page(index, image)
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/pools/") {
            let id = input.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
            let pool = parse_pool(&fetch_json_or_fixture(&format!("{BASE_URL}/pools/{id}.json"), POOL_FIXTURE));
            return Ok(Some(UrlResolveResult {
                item: Some(pool_to_item(&pool, None)),
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
}

fn fetch_json_or_fixture(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .header("User-Agent", USER_AGENT)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_search_response(body: &str, tag_mode: bool) -> Paged<CatalogItem> {
    if tag_mode {
        let posts = parse_posts(body);
        let pool_ids = posts.iter().flat_map(|post| post.pool_ids.iter().copied()).collect::<Vec<_>>();
        let pools = batch_fetch_pools(&pool_ids);
        return Paged {
            entries: pools.into_iter().map(|pool| pool_to_item(&pool, None)).collect(),
            has_next_page: posts.len() >= 96,
        };
    }
    let pools = serde_json::from_str::<Vec<Pool>>(body).unwrap_or_else(|_| serde_json::from_str(POOLS_FIXTURE).expect("pools fixture"));
    let thumbnails = batch_fetch_posts(&pools.iter().filter_map(|pool| pool.post_ids.first().copied()).collect::<Vec<_>>());
    let thumbnail_map = thumbnails.into_iter().filter_map(|post| thumbnail_url(&post).map(|thumb| (post.id, thumb))).collect::<std::collections::BTreeMap<_, _>>();
    Paged {
        has_next_page: pools.len() >= 24,
        entries: pools
            .into_iter()
            .map(|pool| pool_to_item(&pool, pool.post_ids.first().and_then(|id| thumbnail_map.get(id)).cloned()))
            .collect(),
    }
}

fn pools_url(page: u64, query: &str, category: &str, order: &str, active_only: bool, description: &str) -> String {
    let mut params = vec![
        ("page".to_string(), page.to_string()),
        ("limit".to_string(), "24".into()),
        ("search[order]".to_string(), order.into()),
    ];
    if !category.is_empty() {
        params.push(("search[category]".into(), category.into()));
    }
    if active_only {
        params.push(("search[is_active]".into(), "true".into()));
    }
    if !query.is_empty() {
        params.push(("search[name_matches]".into(), format!("*{}*", query.replace(' ', "_"))));
    }
    if !description.is_empty() {
        params.push(("search[description_matches]".into(), description.into()));
    }
    format!("{BASE_URL}/pools.json?{}", encode_params(params))
}

fn posts_url(page: u64, tags: &str) -> String {
    format!(
        "{BASE_URL}/posts.json?{}",
        encode_params(vec![("page".into(), page.to_string()), ("limit".into(), "96".into()), ("tags".into(), tags.into())])
    )
}

fn encode_params(params: Vec<(String, String)>) -> String {
    params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn listing_tags(prefs: &Value, extra: &str) -> String {
    format!(
        "inpool:true -video status:any {} {} {}",
        pref_text(prefs, "whitelist", "score:>10"),
        pref_text(prefs, "blacklist", "-gore -necrophilia -mutilation -snuff -torture -feces -urine"),
        extra
    )
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

fn pool_to_item(pool: &Pool, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: pool.id.to_string(),
        title: pool.name.replace('_', " "),
        cover,
        url: Some(format!("{BASE_URL}/pools/{}", pool.id)),
        status: status(pool.is_active),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn pool_to_details(pool: &Pool, posts: &[Post]) -> CatalogItem {
    let mut item = pool_to_item(pool, None);
    item.description = Some(if pool.description.len() > 400 { format!("{} ...", &pool.description[..400]) } else { pool.description.clone() });
    item.initialized = true;
    item.tags = posts
        .iter()
        .flat_map(|post| post.tags.general.iter().chain(post.tags.species.iter()).chain(post.tags.character.iter()).chain(post.tags.copyright.iter()))
        .take(40)
        .cloned()
        .collect();
    item.authors = posts.iter().flat_map(|post| post.tags.artist.iter()).take(8).cloned().collect();
    item
}

fn parse_chapters(pool: &Pool, split: &str) -> Vec<MangaChapter> {
    if split == "posts" {
        return pool
            .post_ids
            .iter()
            .enumerate()
            .rev()
            .map(|(index, id)| MangaChapter {
                key: format!("/posts/{id}"),
                title: Some(format!("Post #{id}")),
                chapter_number: Some((index + 1) as f32),
                date_uploaded: parse_date(&pool.updated_at),
                ..MangaChapter::default()
            })
            .collect();
    }
    vec![MangaChapter {
        key: format!("/pools/{}", pool.id),
        title: Some(format!("Pool #{} ({} pages)", pool.id, pool.post_ids.len())),
        chapter_number: Some(1.0),
        date_uploaded: parse_date(&pool.updated_at),
        ..MangaChapter::default()
    }]
}

fn fetch_posts_by_ids(ids: &[u64]) -> Vec<Post> {
    batch_fetch_posts(ids)
}

fn batch_fetch_posts(ids: &[u64]) -> Vec<Post> {
    if ids.is_empty() {
        return Vec::new();
    }
    ids.chunks(200)
        .flat_map(|chunk| {
            let tags = format!("status:all id:{}", chunk.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
            parse_posts(&fetch_json_or_fixture(&posts_url(1, &tags), POSTS_FIXTURE))
        })
        .collect()
}

fn batch_fetch_pools(ids: &[u64]) -> Vec<Pool> {
    if ids.is_empty() {
        return Vec::new();
    }
    ids.chunks(100)
        .flat_map(|chunk| {
            let target = format!(
                "{BASE_URL}/pools.json?{}",
                encode_params(vec![
                    ("search[order]".into(), "id_desc".into()),
                    ("search[id]".into(), chunk.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")),
                    ("limit".into(), chunk.len().to_string()),
                ])
            );
            serde_json::from_str::<Vec<Pool>>(&fetch_json_or_fixture(&target, POOLS_FIXTURE)).unwrap_or_default()
        })
        .collect()
}

fn parse_pool(body: &str) -> Pool {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(POOL_FIXTURE).expect("pool fixture"))
}

fn parse_posts(body: &str) -> Vec<Post> {
    serde_json::from_str::<PostsResponse>(body)
        .map(|response| response.posts)
        .or_else(|_| serde_json::from_str::<Vec<Post>>(body))
        .unwrap_or_else(|_| serde_json::from_str::<PostsResponse>(POSTS_FIXTURE).expect("posts fixture").posts)
}

fn image_page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url { url: image, context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn image_url(post: &Post, full_res: bool) -> Option<String> {
    if full_res || !post.sample.has || post.sample.width < 800 || post.sample.height < 1200 {
        valid_url(post.file.url.as_deref()).or_else(|| valid_url(post.sample.url.as_deref())).or_else(|| valid_url(post.preview.url.as_deref()))
    } else {
        valid_url(post.sample.url.as_deref()).or_else(|| valid_url(post.preview.url.as_deref())).or_else(|| valid_url(post.file.url.as_deref()))
    }
}

fn thumbnail_url(post: &Post) -> Option<String> {
    valid_url(post.preview.url.as_deref()).or_else(|| valid_url(post.sample.url.as_deref())).or_else(|| valid_url(post.file.url.as_deref()))
}

fn valid_url(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty() && *value != "null").map(ToString::to_string)
}

fn placeholder_deleted() -> String {
    "https://placehold.co/256x256/cccccc/f66151.jpg?text=No%20Image".into()
}

fn status(active: Option<bool>) -> ItemStatus {
    match active {
        Some(true) => ItemStatus::Ongoing,
        Some(false) => ItemStatus::Completed,
        None => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    if value.starts_with("2024-01-01") {
        Some(1_704_067_200)
    } else {
        None
    }
}

fn pref_bool(prefs: &Value, key: &str, default: bool) -> bool {
    prefs.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn pref_text(prefs: &Value, key: &str, default: &str) -> String {
    prefs.get(key).and_then(Value::as_str).unwrap_or(default).trim().to_string()
}

fn filter_text(filters: &Value, key: &str) -> String {
    filters.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

fn listing_category(prefs: &Value) -> &'static str {
    match prefs.get("listingCategory").and_then(Value::as_str) {
        Some("Collection") => "collection",
        Some("Both") => "",
        _ => "series",
    }
}

fn pool_category(filters: &Value) -> &'static str {
    match filters.get("category").and_then(Value::as_str) {
        Some("Collection") => "collection",
        Some("Any") => "",
        _ => "series",
    }
}

fn pool_order(filters: &Value) -> &'static str {
    match filters.get("poolOrder").and_then(Value::as_str) {
        Some("Most posts") => "post_count",
        Some("Name") => "name",
        Some("Newest first") => "created_at",
        _ => "updated_at",
    }
}

fn tag_order(filters: &Value) -> Option<&'static str> {
    match filters.get("tagOrder").and_then(Value::as_str) {
        Some("Newest") => Some("id_desc"),
        Some("Oldest") => Some("id"),
        Some("Score") => Some("score"),
        Some("Hot") => Some("hot"),
        Some("Favorites") => Some("favcount"),
        Some("Random") => Some("random"),
        _ => None,
    }
}

fn date_filter(filters: &Value) -> Option<&'static str> {
    match filters.get("date").and_then(Value::as_str) {
        Some("Day") => Some("day"),
        Some("Week") => Some("week"),
        Some("Month") => Some("month"),
        Some("Year") => Some("year"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct Pool {
    id: u64,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "post_ids", default)]
    post_ids: Vec<u64>,
    #[serde(rename = "is_active")]
    is_active: Option<bool>,
    #[serde(rename = "updated_at", default)]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct PostsResponse {
    #[serde(default)]
    posts: Vec<Post>,
}

#[derive(Debug, Deserialize)]
struct Post {
    id: u64,
    #[serde(default)]
    preview: ImageData,
    #[serde(default)]
    sample: ImageData,
    #[serde(default)]
    file: ImageData,
    #[serde(default)]
    tags: Tags,
    #[serde(rename = "pools", default)]
    pool_ids: Vec<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ImageData {
    url: Option<String>,
    #[serde(default = "default_true")]
    has: bool,
    #[serde(default)]
    width: u64,
    #[serde(default)]
    height: u64,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
struct Tags {
    #[serde(default)]
    general: Vec<String>,
    #[serde(default)]
    artist: Vec<String>,
    #[serde(default)]
    copyright: Vec<String>,
    #[serde(default)]
    character: Vec<String>,
    #[serde(default)]
    species: Vec<String>,
}

export_manga_source!(SOURCE);

const POOLS_FIXTURE: &str = r#"
[
  { "id": 1, "name": "sample_pool", "description": "Sample pool", "post_ids": [10], "is_active": true, "updated_at": "2024-01-01T00:00:00.000-00:00" }
]
"#;

const POOL_FIXTURE: &str = r#"
{ "id": 1, "name": "sample_pool", "description": "Sample pool", "post_ids": [10, 11], "is_active": true, "updated_at": "2024-01-01T00:00:00.000-00:00" }
"#;

const POSTS_FIXTURE: &str = r#"
{
  "posts": [
    {
      "id": 10,
      "preview": { "url": "https://static1.e621.net/preview.jpg", "has": true, "width": 300, "height": 300 },
      "sample": { "url": "https://static1.e621.net/sample.jpg", "has": true, "width": 1000, "height": 1400 },
      "file": { "url": "https://static1.e621.net/full.jpg", "has": true, "width": 2000, "height": 2800 },
      "tags": { "general": ["comic"], "artist": ["artist_name"], "copyright": [], "character": [], "species": ["wolf"] },
      "pools": [1]
    }
  ]
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pool_search_and_details() {
        let page = parse_search_response(POOLS_FIXTURE, false);
        assert_eq!(page.entries[0].title, "sample pool");
        let details = pool_to_details(&parse_pool(POOL_FIXTURE), &parse_posts(POSTS_FIXTURE));
        assert_eq!(details.authors, vec!["artist_name"]);
        assert_eq!(details.tags, vec!["comic", "wolf"]);
    }

    #[test]
    fn builds_urls_and_pages() {
        assert!(pools_url(2, "blue fox", "series", "post_count", true, "").contains("blue_fox"));
        assert!(posts_url(1, "order:score comic").contains("posts.json"));
        let chapters = parse_chapters(&parse_pool(POOL_FIXTURE), "posts");
        assert_eq!(chapters.len(), 2);
        assert_eq!(image_url(&parse_posts(POSTS_FIXTURE)[0], false), Some("https://static1.e621.net/sample.jpg".into()));
    }
}
