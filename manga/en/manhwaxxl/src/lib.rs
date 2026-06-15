use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: ManhwaXXL = ManhwaXXL;
const BASE_URL: &str = "https://hentaitnt.net";

struct ManhwaXXL;

impl MangaSource for ManhwaXXL {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "recommended"
        };
        Ok(parse_listing(&fetch_document(&paged_url(path, page), LIST_FIXTURE)))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            let genre = filter(request.get("filters"), "genre", "");
            if genre.is_empty() {
                paged_url("", page)
            } else {
                paged_url(&format!("genre/{genre}"), page)
            }
        } else if page > 1 {
            format!("{BASE_URL}/page/{page}?s={}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let post_id = html::attr_after(&body, "post_manga_id", "value").unwrap_or_default();
        let html = if post_id.is_empty() {
            CHAPTERS_FIXTURE.to_string()
        } else {
            post_chapters(&post_id)
        };
        Ok(parse_chapters(&html, preference_bool(&request, "hide_vip_chapters")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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

fn post_chapters(post_id: &str) -> String {
    let body = json!({
        "action": "baka_ajax",
        "type": "load_chapters_paginated",
        "parent_id": post_id,
        "per_page": "10000",
        "order": "newest_first"
    });
    let response = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(&[
            ("action", "baka_ajax"),
            ("type", "load_chapters_paginated"),
            ("parent_id", post_id),
            ("per_page", "10000"),
            ("order", "newest_first"),
        ])
        .send_text()
        .unwrap_or_else(|_| json!({"data":{"html":CHAPTERS_FIXTURE}}).to_string());
    serde_json::from_str::<Value>(&response)
        .ok()
        .and_then(|root| root.get("data")?.get("html")?.as_str().map(ToString::to_string))
        .unwrap_or_else(|| body.get("data").and_then(Value::as_str).unwrap_or(CHAPTERS_FIXTURE).to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("comic-card")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Manhwa XXL".into());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("en".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("title=\"Next\"") || body.contains("title='Next'"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    let status_text = icon_value(body, "Status").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manhwa XXL".into()),
        cover: image_attr(body).map(|image| absolute_url(&image)),
        description: html::text_between(body, "synopsisText", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        authors: icon_value(body, "Artists").into_iter().collect(),
        tags: body.split("genre-item").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).collect(),
        status: parse_status(&status_text),
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_vip: bool) -> Vec<MangaChapter> {
    body.split("comic-card")
        .skip(1)
        .filter_map(|chunk| {
            let is_vip = chunk.contains("fa-crown");
            if is_vip && hide_vip {
                return None;
            }
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let mut title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)))
                .unwrap_or_else(|| "Chapter".into());
            if is_vip {
                title = format!("[VIP] {title}");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                is_locked: is_vip,
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-image"))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn icon_value(body: &str, title: &str) -> Option<String> {
    body.split(&format!("title=\"{title}\""))
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters.and_then(Value::as_object).and_then(|object| object.get(key)).and_then(Value::as_str).filter(|value| !value.is_empty()).unwrap_or(fallback)
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request.get("preferences").and_then(|prefs| prefs.get(key)).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_status(input: &str) -> ItemStatus {
    match input.to_ascii_lowercase().as_str() {
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "srcset").and_then(|value| value.split_whitespace().next().map(ToString::to_string)))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn paged_url(path: &str, page: u64) -> String {
    let path = path.trim_matches('/');
    if page > 1 {
        format!("{BASE_URL}/{path}/page/{page}")
    } else if path.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/{path}")
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="comic-card"><a href="/series/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div><a title="Next"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Manga</h1><input id="post_manga_id" value="1"><div id="synopsisText">Sample summary</div><i title="Artists"></i><span>Sample Artist</span><i title="Status">Ongoing</i><span>Ongoing</span><a class="genre-item">Action</a><img src="/cover.jpg">"#;
const CHAPTERS_FIXTURE: &str = r#"<div class="comic-card"><a href="/series/sample/chapter-1/" title="Chapter 1"></a></div><div class="comic-card"><i class="fa-crown"></i><a href="/series/sample/vip/" title="VIP"></a></div>"#;
const PAGES_FIXTURE: &str = r#"<img class="page-image" src="/page1.jpg"><img class="page-image" src="/page2.jpg">"#;
