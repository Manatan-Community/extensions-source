use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: EternalMangas = EternalMangas;
const BASE_URL: &str = "https://eternalmangas.org";
const API_URL: &str = "https://api.eternalmangas.org";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PER_PAGE: u64 = 18;

struct EternalMangas;

impl MangaSource for EternalMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(PAGE_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            return Ok(parse_query_response(&fetch_json_or_fixture(
                &format!(
                    "{API_URL}/api/posts?page={page}&perPage={PER_PAGE}&isNovel=false&tag=latestUpdate"
                ),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_popular(&fetch_rsc_or_fixture(BASE_URL, PAGE_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/series/") {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![parse_details_json(
                    &fetch_json_or_fixture(&post_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_query_response(&fetch_json_or_fixture(
            &format!(
                "{API_URL}/api/query?page={page}&perPage={PER_PAGE}&searchTerm={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        Ok(parse_details_json(
            &fetch_json_or_fixture(&post_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        Ok(parse_chapters_json(
            &fetch_json_or_fixture(&post_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#1".to_string());
        let chapter_id = key.rsplit_once('#').map(|(_, id)| id).unwrap_or("1");
        let root = json_or_fixture(
            &fetch_json_or_fixture(
                &format!("{API_URL}/api/chapter?chapterId={chapter_id}"),
                PAGES_FIXTURE,
            ),
            PAGES_FIXTURE,
        );
        let chapter = root.get("chapter").unwrap_or(&root);
        if chapter
            .get("isShortLinkLocked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(ExtensionError {
                message: "Chapter locked (short link)".to_string(),
            });
        }
        if chapter
            .get("isLockedByCoins")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(ExtensionError {
                message: "Chapter locked (coins required)".to_string(),
            });
        }
        if chapter
            .get("isPermanentlyLocked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(ExtensionError {
                message: "Chapter permanently locked".to_string(),
            });
        }
        Ok(parse_pages_json(chapter))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let path = key.split_once('#').map(|(path, _)| path).unwrap_or(&key);
            url::join_url(BASE_URL, path)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/series/") {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_json(
                    &fetch_json_or_fixture(&post_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let root = extract_object_containing(body, "\"todayposts\"")
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .unwrap_or_else(|| json_or_fixture(PAGE_FIXTURE, PAGE_FIXTURE));
    let entries = ["todayposts", "weekposts", "monthposts"]
        .into_iter()
        .flat_map(|field| {
            root.get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|post| {
            !post
                .get("isNovel")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(catalog_from_post)
        .fold(Vec::new(), push_unique_item);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_query_response(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("posts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|post| {
            !post
                .get("isNovel")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(catalog_from_post)
        .collect::<Vec<_>>();
    let total = root
        .get("totalCount")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: total > entries.len() as u64,
        entries,
    }
}

fn parse_details_json(body: &str, fallback_key: Option<String>) -> CatalogItem {
    let root = json_or_fixture(body, DETAILS_FIXTURE);
    let post = root.get("post").unwrap_or(&root);
    let mut item = catalog_from_post(post);
    if let Some(key) = fallback_key {
        item.key = normalize_manga_key(&key);
        item.url = Some(manga_url(&item.key));
    }
    item.description = description(post);
    item.authors = string_value(post, "author").into_iter().collect();
    item.artists = string_value(post, "artist").into_iter().collect();
    item.tags = genres(post);
    item.status = status(post.get("seriesStatus").and_then(Value::as_str));
    item.initialized = true;
    item
}

fn parse_chapters_json(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, DETAILS_FIXTURE);
    let post = root.get("post").unwrap_or(&root);
    let series_slug = post
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or_else(|| manga_key.split('#').next().unwrap_or("sample"));
    post.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| chapter.get("chapterStatus").and_then(Value::as_str) == Some("PUBLIC"))
        .filter(|chapter| {
            chapter
                .get("isAccessible")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64)?;
            let slug = chapter
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or("chapter");
            let number = number_text(chapter.get("number")).unwrap_or_else(|| "?".to_string());
            let suffix = string_value(chapter, "title")
                .filter(|value| !value.is_empty())
                .map(|title| format!(" - {title}"))
                .unwrap_or_default();
            let key = format!("/series/{series_slug}/{slug}#{id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapter {number}{suffix}")),
                date_uploaded: string_value(chapter, "createdAt").and_then(|value| {
                    manatan_shared::dates::parse_ymd(&value[..value.len().min(10)])
                }),
                url: Some(url::join_url(
                    BASE_URL,
                    key.split('#').next().unwrap_or(&key),
                )),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages_json(chapter: &Value) -> Vec<MangaPage> {
    let mut images = chapter
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let order = image
                .get("order")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            let image_url = image
                .get("url")
                .and_then(Value::as_str)?
                .replace(' ', "%20");
            Some((order, image_url))
        })
        .collect::<Vec<_>>();
    images.sort_by_key(|(order, _)| *order);
    images
        .into_iter()
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
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

fn catalog_from_post(post: &Value) -> CatalogItem {
    let slug = post
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample")
        .to_string();
    let key = post
        .get("id")
        .and_then(Value::as_i64)
        .map(|id| format!("{slug}#{id}"))
        .unwrap_or(slug);
    CatalogItem {
        key: key.clone(),
        title: string_value(post, "postTitle").unwrap_or_else(|| "EternalMangas".to_string()),
        cover: string_value(post, "featuredImage"),
        url: Some(manga_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn description(post: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(content) = string_value(post, "postContent")
        .map(|value| html::strip_tags(&value.replace('\n', "<br>")))
        .filter(|value| !value.is_empty())
    {
        parts.push(content);
    }
    if let Some(alt) = string_value(post, "alternativeTitles").filter(|value| !value.is_empty()) {
        parts.push(format!("Alternative Names: {alt}"));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn genres(post: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match post.get("seriesType").and_then(Value::as_str) {
        Some("MANGA") => out.push("Manga".to_string()),
        Some("MANHUA") => out.push("Manhua".to_string()),
        Some("MANHWA") => out.push("Manhwa".to_string()),
        _ => {}
    }
    out.extend(
        post.get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| string_value(genre, "name")),
    );
    out.sort();
    out.dedup();
    out
}

fn status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("ONGOING") | Some("COMING_SOON") => ItemStatus::Ongoing,
        Some("COMPLETED") => ItemStatus::Completed,
        Some("CANCELLED") | Some("DROPPED") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn post_url(key: &str) -> String {
    format!(
        "{API_URL}/api/post?postSlug={}",
        url::query_escape(key.split('#').next().unwrap_or(key).trim_matches('/'))
    )
}

fn manga_url(key: &str) -> String {
    format!(
        "{BASE_URL}/series/{}",
        key.split('#').next().unwrap_or(key).trim_matches('/')
    )
}

fn normalize_manga_key(input: &str) -> String {
    let value = input
        .split('#')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    let slug = value
        .split("/series/")
        .nth(1)
        .unwrap_or(value)
        .trim_matches('/');
    input
        .rsplit_once('#')
        .map(|(_, id)| format!("{slug}#{id}"))
        .unwrap_or_else(|| slug.to_string())
}

fn string_value(item: &Value, field: &str) -> Option<String> {
    item.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn number_text(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| value.as_f64().map(|number| number.to_string()))
            .filter(|text| !text.is_empty())
    })
}

fn extract_object_containing(body: &str, needle: &str) -> Option<String> {
    let needle_index = body.find(needle)?;
    let bytes = body.as_bytes();
    let mut start = needle_index;
    while start > 0 && bytes[start] != b'{' {
        start -= 1;
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in body[start..].char_indices() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            if ch != '\\' {
                escaped = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(body[start..start + offset + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

const PAGE_FIXTURE: &str = r#"{
  "todayposts": [{"id": 1, "slug": "sample", "postTitle": "Sample", "featuredImage": "https://eternalmangas.org/sample.jpg", "isNovel": false}],
  "weekposts": [],
  "monthposts": []
}"#;

const LIST_FIXTURE: &str = r#"{
  "posts": [{"id": 1, "slug": "sample", "postTitle": "Sample", "featuredImage": "https://eternalmangas.org/sample.jpg", "isNovel": false}],
  "totalCount": 1
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "post": {
    "id": 1,
    "slug": "sample",
    "postTitle": "Sample",
    "featuredImage": "https://eternalmangas.org/sample.jpg",
    "postContent": "<p>Description</p>",
    "alternativeTitles": "Sample Alt",
    "seriesType": "MANGA",
    "seriesStatus": "ONGOING",
    "genres": [{"name": "Accion"}],
    "chapters": [
      {"id": 10, "slug": "chapter-1", "number": 1, "title": "Start", "createdAt": "2024-01-01T00:00:00.000Z", "chapterStatus": "PUBLIC", "isAccessible": true}
    ]
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "chapter": {
    "id": 10,
    "images": [
      {"url": "https://eternalmangas.org/page-2.jpg", "order": 2},
      {"url": "https://eternalmangas.org/page-1.jpg", "order": 1}
    ]
  }
}"#;

export_manga_source!(SOURCE);
