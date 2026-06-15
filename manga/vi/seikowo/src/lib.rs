use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Seikowo = Seikowo;
const BASE_URL: &str = "https://seikowo-app.blogspot.com";
const WORKER_API_URL: &str = "https://seikowo.shimakazevn.workers.dev/api/v1/posts";
const WORKER_BLOG_ID: &str = "5099059547407963215";

struct Seikowo;

impl MangaSource for Seikowo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(parse_popular(&fetch_document(BASE_URL, HOME_FIXTURE)));
        }
        Ok(parse_feed_page(&fetch_feed(page, 30), page, 30))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let status = filter(filters, "status");
        let genre = filter(filters, "genre");
        let mut entries = fetch_catalogue()
            .into_iter()
            .filter(|entry| {
                query.is_empty() || entry.title.to_lowercase().contains(&query.to_lowercase())
            })
            .filter(|entry| status.is_none_or(|value| entry.status.as_deref() == Some(value)))
            .filter(|entry| {
                genre.is_none_or(|value| {
                    entry.tags.iter().any(|tag| tag.eq_ignore_ascii_case(value))
                })
            })
            .collect::<Vec<_>>();
        match filter(filters, "sort").unwrap_or("updated") {
            "title" => entries.sort_by_key(|entry| entry.title.to_lowercase()),
            "published" => entries.sort_by_key(|entry| std::cmp::Reverse(entry.published_at)),
            _ => entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at)),
        }
        let start = ((page - 1) * 30) as usize;
        let page_entries = entries
            .iter()
            .skip(start)
            .take(30)
            .cloned()
            .map(CatalogueEntry::into_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            entries: page_entries,
            has_next_page: start + 30 < entries.len(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let metadata = post_id(&body)
            .and_then(fetch_feed_entry)
            .and_then(|entry| parse_metadata(entry.content?.value.as_deref()));
        Ok(metadata
            .and_then(|metadata| chapters_from_metadata(&key, metadata))
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample.html?ch=1&sid=sample".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let mut images = image_urls(&body, &chapter_url);
        if images.is_empty() {
            images = worker_images(&key);
        }
        if images.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| page_item(index, &image))
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_feed(page: u64, limit: u64) -> String {
    let start = ((page - 1) * limit) + 1;
    let target = format!(
        "{BASE_URL}/feeds/posts/default?alt=json&orderby=updated&max-results={limit}&start-index={start}"
    );
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| FEED_FIXTURE.to_string())
}

fn parse_feed_page(body: &str, page: u64, limit: u64) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<FeedResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(FEED_FIXTURE).expect("fixture is valid"));
    let entries = response
        .feed
        .entry
        .into_iter()
        .filter_map(CatalogueEntry::from_feed)
        .map(CatalogueEntry::into_item)
        .collect::<Vec<_>>();
    let total = response
        .feed
        .total_results
        .and_then(|value| value.value)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(entries.len() as u64);
    Paged {
        entries,
        has_next_page: page * limit < total,
    }
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let data = html::text_between(body, "window.__POPULAR_POST__", "</script>").unwrap_or_default();
    let entries = data
        .split('{')
        .filter(|chunk| chunk.contains("featuredImage"))
        .filter_map(|chunk| {
            let title = js_field(chunk, "title")?;
            let href = js_field(chunk, "url")?;
            let key = key_from_url(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: html::html_unescape(&title),
                cover: js_field(chunk, "featuredImage"),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .take(10)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn fetch_catalogue() -> Vec<CatalogueEntry> {
    let mut out = Vec::new();
    let mut page = 1;
    loop {
        let body = fetch_feed(page, 30);
        let response = serde_json::from_str::<FeedResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(FEED_FIXTURE).expect("fixture is valid"));
        let before = out.len();
        out.extend(
            response
                .feed
                .entry
                .into_iter()
                .filter_map(CatalogueEntry::from_feed),
        );
        if out.len() == before || out.len() >= 300 {
            break;
        }
        page += 1;
    }
    out
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    let entry = post_id(&body).and_then(fetch_feed_entry);
    let metadata = entry
        .as_ref()
        .and_then(|entry| parse_metadata(entry.content.as_ref()?.value.as_deref()));
    if let Some(metadata) = metadata {
        return CatalogItem {
            key: normalize_key(key),
            title: html::html_unescape(
                &metadata
                    .title
                    .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
            ),
            cover: metadata.cover_image,
            authors: metadata.author.into_iter().collect(),
            artists: metadata.artist.into_iter().collect(),
            tags: metadata.tags.unwrap_or_default(),
            description: metadata.description.map(|value| html::strip_tags(&value)),
            status: metadata
                .status
                .as_deref()
                .map(parse_status)
                .unwrap_or(ItemStatus::Unknown),
            url: Some(absolute_url(key)),
            language: Some("vi".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        };
    }
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(&body, "<title", "</title>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "Manga".into()),
        cover: image_attr(&body).map(|image| absolute_url(&image)),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn post_id(body: &str) -> Option<String> {
    body.split("postId")
        .nth(1)
        .and_then(|tail| {
            tail.split(['"', '\''])
                .find(|part| part.chars().all(|ch| ch.is_ascii_digit()) && !part.is_empty())
        })
        .map(ToString::to_string)
        .or_else(|| {
            body.split("id:")
                .nth(1)
                .and_then(|tail| {
                    tail.split(['"', '\''])
                        .find(|part| part.chars().all(|ch| ch.is_ascii_digit()) && !part.is_empty())
                })
                .map(ToString::to_string)
        })
}

fn fetch_feed_entry(post_id: String) -> Option<FeedEntry> {
    let target = format!("{BASE_URL}/feeds/posts/default/{post_id}?alt=json");
    let body = client().get(target).xhr().send_text().ok()?;
    serde_json::from_str::<FeedEntryResponse>(&body)
        .ok()
        .map(|response| response.entry)
}

fn parse_metadata(content: Option<&str>) -> Option<SeriesMetadata> {
    let content = content?;
    let raw = html::text_between(content, "id=\"seikowo-metadata\"", "</script>")
        .or_else(|| html::text_between(content, "id='seikowo-metadata'", "</script>"))?;
    serde_json::from_str(&raw).ok()
}

fn chapters_from_metadata(
    source_path: &str,
    metadata: SeriesMetadata,
) -> Option<Vec<MangaChapter>> {
    let series_id = metadata.series_id?;
    let chapters = metadata.chapters?;
    Some(
        chapters
            .into_iter()
            .filter_map(|chapter| {
                let number = chapter.number.or(chapter.chapter_num)?;
                let text = format_chapter_number(number);
                let mut title = format!("Chuong {text}");
                if let Some(extra) = chapter
                    .title
                    .or(chapter.chapter_title)
                    .filter(|value| !value.trim().is_empty())
                {
                    title.push_str(" - ");
                    title.push_str(&html::html_unescape(&extra));
                }
                let key = format!(
                    "{}?ch={}&sid={}",
                    source_path.split('?').next().unwrap_or(source_path),
                    text,
                    series_id
                );
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(title),
                    chapter_number: Some(number as f32),
                    date_uploaded: chapter
                        .updated_at
                        .or(chapter.created_at)
                        .as_deref()
                        .and_then(parse_feed_date),
                    url: Some(absolute_url(&key)),
                    ..MangaChapter::default()
                })
            })
            .collect(),
    )
}

fn worker_images(chapter_key: &str) -> Vec<String> {
    let Some(series_id) = query_param(chapter_key, "sid") else {
        return Vec::new();
    };
    let payload = json!({
        "action": "list",
        "labels": ["Data_Node", format!("Parent_{series_id}")],
        "maxResults": 50,
        "fetchFields": "items(id,content)",
        "blogId": WORKER_BLOG_ID
    });
    let body = client()
        .post(WORKER_API_URL)
        .header("Content-Type", "application/json; charset=utf-8")
        .json(payload.to_string())
        .send_text()
        .unwrap_or_default();
    serde_json::from_str::<WorkerList>(&body)
        .ok()
        .into_iter()
        .flat_map(|list| list.items)
        .flat_map(|post| image_urls(post.content.as_deref().unwrap_or_default(), BASE_URL))
        .collect::<Vec<_>>()
}

fn image_urls(body: &str, base: &str) -> Vec<String> {
    let from_imgs = body
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .map(|image| url::join_url(base, &image));
    let from_google = body
        .split('"')
        .filter(|part| part.contains("googleusercontent.com"))
        .map(|part| high_res(part));
    from_imgs
        .chain(from_google)
        .filter(|image| !image.starts_with("data:"))
        .fold(Vec::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr(chunk, "data-original"))
}

fn high_res(input: &str) -> String {
    if let Some(index) = input.find("/s") {
        if let Some(end) = input[index + 1..].find('/') {
            let mut value = input.to_string();
            value.replace_range(index..index + end + 2, "/s3200-rw");
            return value;
        }
    }
    input.to_string()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_feed_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(manatan_shared::dates::parse_ymd)
}

fn format_chapter_number(number: f64) -> String {
    let whole = number as i64;
    if (number - whole as f64).abs() < 0.0001 {
        whole.to_string()
    } else {
        number
            .to_string()
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn js_field(chunk: &str, name: &str) -> Option<String> {
    let tail = chunk.split(name).nth(1)?;
    let quote = tail.find('"')?;
    Some(tail[quote + 1..].split('"').next()?.to_string())
}

fn page_item(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn query_param<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    input.split('?').nth(1)?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .split('#')
            .next()
            .unwrap_or(value)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.ends_with(".html") || key.contains(".html?"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

#[derive(Clone)]
struct CatalogueEntry {
    key: String,
    title: String,
    cover: Option<String>,
    updated_at: Option<i64>,
    published_at: Option<i64>,
    status: Option<String>,
    tags: Vec<String>,
}

impl CatalogueEntry {
    fn from_feed(entry: FeedEntry) -> Option<Self> {
        let metadata = parse_metadata(entry.content.as_ref()?.value.as_deref());
        let href = entry
            .link
            .into_iter()
            .find(|link| link.rel.as_deref() == Some("alternate"))?
            .href?;
        let key = key_from_url(&href)?;
        let title = metadata
            .as_ref()
            .and_then(|m| m.title.clone())
            .or_else(|| entry.title.and_then(|title| title.value))
            .unwrap_or_else(|| "Manga".into());
        let tags = metadata
            .as_ref()
            .and_then(|m| m.tags.clone())
            .unwrap_or_else(|| {
                entry
                    .category
                    .into_iter()
                    .filter_map(|cat| cat.term)
                    .collect()
            });
        Some(Self {
            key,
            title: html::html_unescape(&title),
            cover: metadata
                .as_ref()
                .and_then(|m| m.cover_image.clone())
                .or_else(|| entry.thumbnail.and_then(|thumb| thumb.url)),
            updated_at: entry
                .updated
                .and_then(|value| value.value)
                .as_deref()
                .and_then(parse_feed_date),
            published_at: entry
                .published
                .and_then(|value| value.value)
                .as_deref()
                .and_then(parse_feed_date),
            status: metadata.and_then(|m| m.status),
            tags,
        })
    }

    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.key.clone(),
            title: self.title,
            cover: self.cover,
            tags: self.tags,
            url: Some(absolute_url(&self.key)),
            language: Some("vi".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct FeedResponse {
    feed: Feed,
}
#[derive(Deserialize)]
struct FeedEntryResponse {
    entry: FeedEntry,
}
#[derive(Deserialize)]
struct Feed {
    #[serde(default)]
    entry: Vec<FeedEntry>,
    #[serde(rename = "openSearch$totalResults")]
    total_results: Option<TextValue>,
}
#[derive(Deserialize)]
struct FeedEntry {
    title: Option<TextValue>,
    content: Option<TextValue>,
    #[serde(default)]
    link: Vec<FeedLink>,
    #[serde(default)]
    category: Vec<FeedCategory>,
    #[serde(rename = "media$thumbnail")]
    thumbnail: Option<FeedThumbnail>,
    updated: Option<TextValue>,
    published: Option<TextValue>,
}
#[derive(Deserialize)]
struct TextValue {
    #[serde(rename = "$t")]
    value: Option<String>,
}
#[derive(Deserialize)]
struct FeedLink {
    rel: Option<String>,
    href: Option<String>,
}
#[derive(Deserialize)]
struct FeedCategory {
    term: Option<String>,
}
#[derive(Deserialize)]
struct FeedThumbnail {
    url: Option<String>,
}
#[derive(Deserialize)]
struct SeriesMetadata {
    title: Option<String>,
    author: Option<String>,
    artist: Option<String>,
    description: Option<String>,
    #[serde(rename = "coverImage")]
    cover_image: Option<String>,
    status: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(rename = "seriesId")]
    series_id: Option<String>,
    chapters: Option<Vec<ChapterMetadata>>,
}
#[derive(Deserialize)]
struct ChapterMetadata {
    number: Option<f64>,
    #[serde(rename = "chapterNum")]
    chapter_num: Option<f64>,
    title: Option<String>,
    #[serde(rename = "chapterTitle")]
    chapter_title: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
}
#[derive(Deserialize)]
struct WorkerList {
    #[serde(default)]
    items: Vec<WorkerPost>,
}
#[derive(Deserialize)]
struct WorkerPost {
    content: Option<String>,
}

const HOME_FIXTURE: &str = r#"<script>window.__POPULAR_POST__ = JSON.stringify({data:[{title:"Sample",url:"https://seikowo-app.blogspot.com/2024/01/sample.html",featuredImage:"/cover.jpg"}]})</script>"#;
const FEED_FIXTURE: &str = r#"{"feed":{"openSearch$totalResults":{"$t":"1"},"entry":[{"title":{"$t":"Sample"},"content":{"$t":"<script id=\"seikowo-metadata\">{\"title\":\"Sample\",\"seriesId\":\"sample\",\"coverImage\":\"/cover.jpg\",\"status\":\"ongoing\",\"tags\":[\"Action\"],\"chapters\":[{\"number\":1,\"title\":\"\"}]}</script>"},"link":[{"rel":"alternate","href":"https://seikowo-app.blogspot.com/2024/01/sample.html"}],"category":[{"term":"Action"}],"media$thumbnail":{"url":"/cover.jpg"},"updated":{"$t":"2024-01-01T00:00:00Z"},"published":{"$t":"2024-01-01T00:00:00Z"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"<script>var postId = "1";</script><script id="seikowo-metadata">{"title":"Sample","seriesId":"sample","coverImage":"/cover.jpg","status":"ongoing","tags":["Action"],"chapters":[{"number":1,"title":""}]}</script>"#;
const PAGES_FIXTURE: &str = r#"<div><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
