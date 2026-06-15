use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MurimScan = MurimScan;
const BASE_URL: &str = "https://www.murimscans.site";
const MANGA_CATEGORY: &str = "Series";
const CHAPTER_CATEGORY: &str = "Chapter";
const PAGE_SIZE: u64 = 20;

struct MurimScan;

impl MangaSource for MurimScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            return Ok(parse_feed(&fetch_document(&feed_url(MANGA_CATEGORY, "", page, Some("published")), FEED_FIXTURE)));
        }
        Ok(parse_popular(&fetch_document(BASE_URL, HOME_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let label_query = if query.is_empty() {
            filter_label(request.get("filters"))
        } else {
            format!("label:{MANGA_CATEGORY} {}", query)
        };
        Ok(parse_feed(&fetch_document(&feed_url(MANGA_CATEGORY, &label_query, page, None), FEED_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let feed = chapter_feed_url(&body).unwrap_or_else(|| feed_url(CHAPTER_CATEGORY, "", 1, None));
        Ok(parse_chapter_feed(&fetch_document(&feed, CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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

fn feed_url(label: &str, query: &str, page: u64, order: Option<&str>) -> String {
    let start = PAGE_SIZE * page.saturating_sub(1) + 1;
    let mut parts = vec![
        format!("alt=json"),
        format!("max-results={}", PAGE_SIZE + 1),
        format!("start-index={start}"),
    ];
    if let Some(order) = order {
        parts.push(format!("orderby={order}"));
    }
    if !query.is_empty() {
        parts.push(format!("q={}", url::query_escape(query)));
    }
    format!("{BASE_URL}/feeds/posts/default/-/{}?{}", url::query_escape(label), parts.join("&"))
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<figcaption", "</figcaption>")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MurimScan".to_string())),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_feed(body: &str) -> Paged<CatalogItem> {
    let entries = feed_entries(body)
        .into_iter()
        .filter(|entry| entry_has_category(entry, MANGA_CATEGORY) && !entry_has_category(entry, "Anime"))
        .map(item_from_entry)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() as u64 > PAGE_SIZE;
    let entries = entries.into_iter().take(PAGE_SIZE as usize).collect();
    Paged { entries, has_next_page }
}

fn item_from_entry(entry: Value) -> CatalogItem {
    let href = alternate_link(&entry).unwrap_or_else(|| BASE_URL.to_string());
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title: text_field(entry.get("title")).unwrap_or_else(|| "MurimScan".to_string()),
        cover: thumbnail_from_entry(&entry),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let profile = body.split("grid gtc-235fr").nth(1).unwrap_or(body);
    let mut description = html::text_between(profile, "synopsis", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(alt) = html::text_between(profile, "<header", "</header>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative name(s): ");
        description.push_str(&alt);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(profile, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MurimScan".to_string())),
        cover: image_from_chunk(profile),
        authors: text_after_id(profile, "author").into_iter().collect(),
        artists: text_after_id(profile, "artist").into_iter().collect(),
        tags: links_containing(profile, "rel=\"tag\""),
        description: (!description.is_empty()).then_some(description),
        status: parse_status(
            &html::text_between(profile, "data-status", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str) -> Vec<MangaChapter> {
    feed_entries(body)
        .into_iter()
        .filter(|entry| entry_has_category(entry, CHAPTER_CATEGORY))
        .filter_map(|entry| {
            let href = alternate_link(&entry)?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: text_field(entry.get("title")),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let source = body.split("check-box").nth(1).unwrap_or(body);
    source
        .split("separator")
        .skip(1)
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_feed_url(body: &str) -> Option<String> {
    if let Some(script) = body.split("id=\"clwd\"").nth(1) {
        if let Some(feed) = between(script, "clwd.run('", "'").or_else(|| between(script, "clwd.run(\"", "\"")) {
            return Some(format!("{BASE_URL}/feeds/posts/default/-/{CHAPTER_CATEGORY}/{feed}?alt=json"));
        }
    }
    if let Some(script) = body.split("id=\"latest\"").nth(1) {
        if let Some(label) = between(script, "label = '", "'") {
            return Some(feed_url(&label, "", 1, None));
        }
    }
    None
}

fn feed_entries(body: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("feed")?.get("entry")?.as_array().cloned())
        .unwrap_or_default()
}

fn alternate_link(entry: &Value) -> Option<String> {
    entry
        .get("link")?
        .as_array()?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))?
        .get("href")?
        .as_str()
        .map(ToString::to_string)
}

fn entry_has_category(entry: &Value, category: &str) -> bool {
    entry
        .get("category")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.get("term").and_then(Value::as_str) == Some(category)))
}

fn thumbnail_from_entry(entry: &Value) -> Option<String> {
    entry
        .get("media$thumbnail")
        .and_then(|thumb| thumb.get("url"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| text_field(entry.get("content")).and_then(|content| image_from_chunk(&content)))
}

fn text_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|value| value.get("$t"))
        .and_then(Value::as_str)
        .map(|value| html::strip_tags(value))
        .filter(|value| !value.is_empty())
}

fn text_after_id(body: &str, id: &str) -> Option<String> {
    body.split(&format!("id=\"{id}\""))
        .nth(1)
        .map(|chunk| html::strip_tags(chunk.split('<').next().unwrap_or_default()))
        .filter(|value| !value.is_empty())
}

fn links_containing(chunk: &str, needle: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter(|part| part.contains(needle))
        .filter_map(|part| html::text_between(part, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| absolute_url(&image))
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn filter_label(filters: Option<&Value>) -> String {
    filters
        .and_then(|filters| filters.get("label"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn between(input: &str, start: &str, end: &str) -> Option<String> {
    let rest = input.split(start).nth(1)?;
    Some(rest.split(end).next()?.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<figure><a href="/sample"><img src="/cover.jpg"></a><figcaption><a href="/sample">Sample</a></figcaption></figure>"#;
const FEED_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Sample"},"category":[{"term":"Series"}],"link":[{"rel":"alternate","href":"https://www.murimscans.site/sample"}],"media$thumbnail":{"url":"https://www.murimscans.site/cover.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="grid gtc-235fr"><img src="/cover.jpg"><h1>Sample</h1><div id="synopsis">Summary</div><a rel="tag">Action</a><span id="author">Author</span><span id="artist">Artist</span><span data-status>Ongoing</span><div id="clwd"><script>clwd.run('sample')</script></div></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Chapter 1"},"category":[{"term":"Chapter"}],"link":[{"rel":"alternate","href":"https://www.murimscans.site/chapter-1"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<div class="check-box"><div class="separator"><img src="/page1.jpg"></div></div>"#;
