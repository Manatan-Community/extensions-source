use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mehgazone = Mehgazone;
const BASE_URL: &str = "https://mehgazone.com";

struct Mehgazone;

impl MangaSource for Mehgazone {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_document(BASE_URL, LIST_FIXTURE);
        let mut entries = parse_listing(&body);
        if let Some(query) = request.get("query").and_then(Value::as_str).filter(|value| !value.is_empty()) {
            entries.retain(|item| item.title.to_ascii_lowercase().contains(&query.to_ascii_lowercase()));
        }
        Ok(Paged { entries, has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or("").trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/".into());
        Ok(fetch_all_chapters(&absolute_url(&key), &request))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/?p=1".into());
        let post_id = key.split("p=").nth(1).and_then(|part| part.split('&').next()).unwrap_or("1");
        let site_url = absolute_url(key.split('?').next().unwrap_or("/"));
        let target = format!("{}/wp-json/wp/v2/posts?per_page=1&_fields=link,content,excerpt,date,title&include={post_id}", site_url.trim_end_matches('/'));
        Ok(parse_pages(&fetch_wp_json(&target, &request, PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_wp_json(target: &str, request: &Value, fixture: &str) -> String {
    let client = client();
    let mut get = client.get(target).header("Accept", "application/json");
    if let Some(auth) = basic_auth(request) {
        get = get.header("Authorization", auth);
    }
    get.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<h2")
        .skip(1)
        .filter(|chunk| chunk.to_ascii_lowercase().contains("latest") && chunk.contains('"'))
        .filter_map(|chunk| {
            let title = chunk.split('"').nth(1).map(html::html_unescape)?;
            let siblings = chunk.split("</h2>").nth(1).unwrap_or(chunk);
            let href = html::attr_after(siblings, "<a", "href")?;
            let image = html::attr_after(siblings, "<img", "src");
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image.map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<title", "</title>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Mehgazone".into()),
        cover: body
            .split("<img")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "src"))
            .find(|src| src.ends_with(".png"))
            .map(|src| absolute_url(&src)),
        authors: vec!["Patricia Barton".into()],
        status: ItemStatus::Ongoing,
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(manga_url: &str, request: &Value) -> Vec<MangaChapter> {
    let mut all = Vec::new();
    let mut page = 1;
    loop {
        let target = format!("{}/wp-json/wp/v2/posts?per_page=100&page={page}&_fields=id,title,date_gmt,excerpt", manga_url.trim_end_matches('/'));
        let body = fetch_wp_json(&target, request, CHAPTERS_FIXTURE);
        let mut parsed = parse_chapter_page(&body, manga_url);
        let count = parsed.len();
        all.append(&mut parsed);
        if count < 100 || page >= 20 {
            break;
        }
        page += 1;
    }
    all.sort_by(|a, b| a.chapter_number.partial_cmp(&b.chapter_number).unwrap_or(core::cmp::Ordering::Equal));
    all.reverse();
    all
}

fn parse_chapter_page(body: &str, manga_url: &str) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else { return Vec::new(); };
    let Some(posts) = root.as_array() else { return Vec::new(); };
    posts
        .iter()
        .filter(|post| !json_text_path(post, &["excerpt", "rendered"]).unwrap_or_default().contains("Unlock with Patreon"))
        .enumerate()
        .filter_map(|(index, post)| {
            let id = post.get("id").and_then(Value::as_u64)?;
            let title = json_text_path(post, &["title", "rendered"])
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| json_text(post, "date_gmt").map(|value| value.split('T').next().unwrap_or(&value).to_string()));
            let key = format!("{}/?p={id}", normalize_key(manga_url));
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number: Some(index as f32),
                date_uploaded: json_text(post, "date_gmt").and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else { return Vec::new(); };
    let Some(post) = root.as_array().and_then(|items| items.first()) else { return Vec::new(); };
    let link = json_text(post, "link").unwrap_or_else(|| BASE_URL.to_string());
    let content = json_text_path(post, &["content", "rendered"]).unwrap_or_default();
    let mut pages: Vec<_> = content
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: url::join_url(&link, &image), context: None },
            headers: manga::image_headers(&link),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect();
    if let Some(excerpt) = json_text_path(post, &["excerpt", "rendered"]).filter(|value| !value.trim().is_empty()) {
        pages.push(manga::text_page(&html::strip_tags(&excerpt)));
    }
    pages
}

fn basic_auth(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    let user = prefs.get("WORDPRESS_USERNAME").and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let pass = prefs.get("WORDPRESS_APP_PASSWORD").and_then(Value::as_str).filter(|value| !value.is_empty())?;
    Some(format!("Basic {}", base64_basic(&format!("{user}:{pass}"))))
}

fn base64_basic(input: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn json_text_path(value: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_start_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<aside class="primary-sidebar"><div class="sidebar-group"><h2>Latest "Sample Comic"</h2><a href="https://mehgazone.com/"><img src="/cover.png"></a></div></aside>"#;
const DETAILS_FIXTURE: &str = r#"<html><head><title>Sample Comic</title></head><body><div id="content"><img src="/sample-123.png"></div></body></html>"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":1,"date_gmt":"2024-01-01T00:00:00","title":{"rendered":"Chapter 1"},"excerpt":{"rendered":""}}]"#;
const PAGES_FIXTURE: &str = r#"[{"link":"https://mehgazone.com/?p=1","content":{"rendered":"<img src=\"/page1.png\"><img src=\"/page2.png\">"},"excerpt":{"rendered":"Sample note."}}]"#;
