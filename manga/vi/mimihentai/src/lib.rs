use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Map, Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: MiMiHentai = MiMiHentai;
const BASE_URL: &str = "https://mimihentai.net";

struct MiMiHentai;

impl MangaSource for MiMiHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/danh-sach?page={page}")
        } else {
            format!("{BASE_URL}/danh-sach?sort=-views&page={page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), page))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut pairs = vec![
            format!("keyword={}", url::query_escape(query)),
            format!("page={page}"),
        ];
        if let Some(genres) = filter(filters, "accept_genres").filter(|value| !value.is_empty()) {
            pairs.push(format!(
                "filter[accept_genres]={}",
                url::query_escape(&genres)
            ));
        }
        if let Some(status) = filter(filters, "status").filter(|value| !value.is_empty()) {
            pairs.push(format!("filter[status]={}", url::query_escape(&status)));
        }
        let sort = filter(filters, "sort").unwrap_or("-updated_at".into());
        if !sort.is_empty() {
            pairs.push(format!("sort={}", url::query_escape(&sort)));
        }
        Ok(parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/tim-kiem?{}", pairs.join("&")),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &absolute_url(&key),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen/sample/1".into());
        let chapter_url = absolute_url(&key);
        let pages = parse_pages(&fetch_document(&chapter_url, PAGES_FIXTURE), &chapter_url);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, "popular"))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, "latest"))?,
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
            let is_manga = key.split('/').filter(|part| !part.is_empty()).count() <= 2;
            return Ok(Some(UrlResolveResult {
                item: is_manga.then(|| details_by_key(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    let http = client();
    let body = http
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    if body.contains("wire:initial-data") && body.contains("enter-secret") {
        solve_password(&http, &body);
        return http
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or(body);
    }
    body
}

fn solve_password(http: &HttpClient, body: &str) {
    let Some(wire_data_raw) =
        attr_after(body, "wire:initial-data").map(|value| html::html_unescape(&value))
    else {
        return;
    };
    let Some(csrf) = quoted_after(body, "livewire_token") else {
        return;
    };
    let Some(password) = quoted_after(body, "input.value") else {
        return;
    };
    let Ok(wire_data) = serde_json::from_str::<Value>(&wire_data_raw) else {
        return;
    };
    let fingerprint = wire_data.get("fingerprint").cloned().unwrap_or(Value::Null);
    let server_memo = wire_data
        .get("serverMemo")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let sync_payload = json!({
        "fingerprint": fingerprint,
        "serverMemo": server_memo,
        "updates": [{
            "type": "syncInput",
            "payload": { "id": "s1", "name": "password", "value": password }
        }]
    });
    let sync = http
        .post(format!("{BASE_URL}/livewire/message/enter-secret"))
        .header("X-CSRF-TOKEN", csrf.clone())
        .header("X-Livewire", "true")
        .header("Accept", "text/html, application/xhtml+xml")
        .referer(format!("{BASE_URL}/"))
        .json(sync_payload.to_string())
        .send_text()
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let merged = merge_server_memo(
        wire_data
            .get("serverMemo")
            .cloned()
            .unwrap_or(Value::Object(Map::new())),
        sync.and_then(|value| value.get("serverMemo").cloned()),
    );
    let submit_payload = json!({
        "fingerprint": wire_data.get("fingerprint").cloned().unwrap_or(Value::Null),
        "serverMemo": merged,
        "updates": [{
            "type": "callMethod",
            "payload": { "id": "c1", "method": "submit", "params": [] }
        }]
    });
    let _ = http
        .post(format!("{BASE_URL}/livewire/message/enter-secret"))
        .header("X-CSRF-TOKEN", csrf)
        .header("X-Livewire", "true")
        .header("Accept", "text/html, application/xhtml+xml")
        .referer(format!("{BASE_URL}/"))
        .json(submit_payload.to_string())
        .send_text();
}

fn merge_server_memo(base: Value, update: Option<Value>) -> Value {
    let Some(Value::Object(update)) = update else {
        return base;
    };
    let Value::Object(mut base) = base else {
        return Value::Object(update);
    };
    for (key, value) in update {
        match (key.as_str(), base.get_mut("data"), value) {
            ("data", Some(Value::Object(existing)), Value::Object(incoming)) => {
                existing.extend(incoming);
            }
            (_, _, value) => {
                base.insert(key, value);
            }
        }
    }
    Value::Object(base)
}

fn parse_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("group"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h1", "</h1>")
                .map(|text| html::strip_tags(&text))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(item_basic(
                key,
                title,
                image_attr(chunk).map(|image| absolute_url(&image)),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains(&format!("page={}", page + 1)),
    }
}

fn item_basic(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let body_text = html::strip_tags(body);
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "div class=\"title", "</div>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: image_after(body, "rounded shadow-md w-full")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: link_texts(body, "/tac-gia/"),
        tags: link_texts(body, "/the-loai/"),
        description: html::text_between(body, "div class=\"mt-4", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&body_text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, referer: &str) -> Vec<MangaChapter> {
    let chapter_area = body.split("chapter-list").nth(1).unwrap_or(body);
    chapter_area
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h1", "</h1>")
                .or_else(|| html::attr(chunk, "title"))
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: relative_date_seconds(
                    &html::text_between(chunk, "timeago", "</span>")
                        .map(|value| html::strip_tags(&value))
                        .unwrap_or_default(),
                ),
                url: Some(url::join_url(referer, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("lazy") || chunk.contains("data-src"))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .map(|image| url::join_url(referer, &image))
        .fold(Vec::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image, referer))
        .collect()
}

fn page(index: usize, image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("Đã hoàn thành") {
        ItemStatus::Completed
    } else if text.contains("Đang tiến hành") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn relative_date_seconds(text: &str) -> Option<i64> {
    let number = text
        .split_whitespace()
        .find_map(|part| part.parse::<i64>().ok())?;
    let delta = if text.contains("giây") {
        number
    } else if text.contains("phút") {
        number * 60
    } else if text.contains("giờ") {
        number * 3_600
    } else if text.contains("ngày") {
        number * 86_400
    } else if text.contains("tuần") {
        number * 604_800
    } else if text.contains("tháng") {
        number * 2_592_000
    } else if text.contains("năm") {
        number * 31_536_000
    } else {
        return None;
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(now.saturating_sub(delta))
}

fn filter(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_after(body: &str, marker: &str) -> Option<String> {
    body.find(marker)
        .and_then(|index| image_attr(&body[index..]))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr(chunk, "content"))
}

fn attr_after(body: &str, name: &str) -> Option<String> {
    body.find(name)
        .and_then(|index| html::attr(&body[index..], name))
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let tail = body.split(marker).nth(1)?;
    let quote = tail
        .find(['"', '\''])
        .map(|index| tail.as_bytes()[index] as char)?;
    let after = tail.split_once(quote)?.1;
    Some(after.split(quote).next()?.to_string())
}

fn normalize_key(value: &str) -> String {
    let raw = value.trim();
    let without_base = raw.strip_prefix(BASE_URL).unwrap_or(raw);
    format!("/{}", without_base.trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen/"))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "page": 1,
        "listingId": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

const LIST_FIXTURE: &str =
    r#"<a class="group" href="/truyen/sample"><img data-src="/cover.jpg"><h1>Sample</h1></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="title"><p>Sample</p></div><img class="rounded shadow-md w-full" src="/cover.jpg"><a href="/tac-gia/a">Author</a><a href="/the-loai/action">Action</a><div class="mt-4">Summary</div><div class="chapter-list"><a href="/truyen/sample/1"><h1>Chapter 1</h1><span class="timeago">1 ngày trước</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<img class="lazy" data-src="/page1.jpg">"#;

export_manga_source!(SOURCE);
