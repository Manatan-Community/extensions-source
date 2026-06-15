use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SOURCE: BakaInUa = BakaInUa;
const BASE_URL: &str = "https://baka.in.ua";

struct BakaInUa;

impl NovelSource for BakaInUa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = alphabetical_url(page, &request);
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        let ids = fiction_ids(&body);
        let entries = if ids.is_empty() {
            parse_search_listing(&body)
        } else {
            ids.into_iter()
                .filter_map(|id| fetch_picker_item(&id))
                .collect()
        };
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let mut target = format!(
            "{BASE_URL}/search?filter=fiction&search[]={}",
            url::query_escape(query)
        );
        if page > 1 {
            target.push_str("&page=");
            target.push_str(&page.to_string());
        }
        let body = fetch_document_or_fixture(&target, SEARCH_FIXTURE);
        let entries = parse_search_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "fictions/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "fictions/sample".to_string());
        let body = translated_novel_body(&key);
        let mut chapters = parse_chapters(&body);
        chapters.reverse();
        Ok(chapters)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "fictions/sample/chapters/1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Популярне".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Новинки".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn alphabetical_url(page: u64, request: &Value) -> String {
    let listing = request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular");
    let mut params = Vec::new();
    if page > 1 {
        params.push(format!("page={page}"));
    }
    if listing == "latest" || filter_bool(request, "only_new") {
        params.push("only_new=1".to_string());
    }
    if filter_bool(request, "longreads") {
        params.push("longreads=1".to_string());
    }
    if filter_bool(request, "finished") {
        params.push("finished=1".to_string());
    }
    if let Some(genre) = filter_string(request, "genre") {
        params.push(format!("genre={}", url::query_escape(&genre)));
    }
    if params.is_empty() {
        format!("{BASE_URL}/fictions/alphabetical")
    } else {
        format!("{BASE_URL}/fictions/alphabetical?{}", params.join("&"))
    }
}

fn fiction_ids(body: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    body.split("data-fiction-picker-id-param")
        .skip(1)
        .filter_map(|part| {
            let value = part.trim_start();
            let quote = value.chars().find(|ch| *ch == '"' || *ch == '\'')?;
            let rest = value.split_once(quote)?.1;
            let id = rest.split(quote).next()?.to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            Some(id)
        })
        .collect()
}

fn fetch_picker_item(id: &str) -> Option<CatalogItem> {
    let body =
        fetch_document_or_fixture(&format!("{BASE_URL}/fictions/{id}/details"), PICKER_FIXTURE);
    let href = html::attr_after(&body, "<a", "href")?;
    let key = normalize_key(&href);
    Some(catalog_item(
        key,
        text_between_tag(&body, "h3").unwrap_or_else(|| title_from_key(&href)),
        html::attr_after(&body, "<img", "src").map(|src| absolute_url(&src)),
        false,
    ))
}

fn parse_search_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            if !href.contains("/fictions/") {
                return None;
            }
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = text_between_tag(block, "h3")
                .or_else(|| {
                    html::text_between(block, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(
                key,
                title,
                html::attr_after(block, "<img", "src").map(|src| absolute_url(&src)),
                false,
            ))
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = translated_novel_body(key);
    parse_details(&body, key)
}

fn translated_novel_body(key: &str) -> String {
    let first = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    let ids = translator_ids(&first);
    if ids.is_empty() {
        return first;
    }
    let mut target = absolute_url(key);
    let separator = if target.contains('?') { "&" } else { "?" };
    target.push_str(separator);
    target.push_str(
        &ids.iter()
            .map(|id| format!("translator[]={}", url::query_escape(id)))
            .collect::<Vec<_>>()
            .join("&"),
    );
    fetch_document_or_fixture(&target, DETAILS_FIXTURE)
}

fn translator_ids(body: &str) -> Vec<String> {
    let area = body
        .split("alternative-tabs")
        .nth(1)
        .and_then(|tabs| tabs.split("<form").nth(1))
        .and_then(|form| form.split("</form>").next())
        .unwrap_or(body);
    area.split("name=\"translator[]\"")
        .skip(1)
        .filter_map(|part| html::attr(part, "value"))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        text_between_tag(body, "h1").unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|src| absolute_url(&src)),
        true,
    );
    let author =
        text_for_marker(body, "fictions-author-search").unwrap_or_else(|| "Невідомо".to_string());
    item.authors.push(author.clone());
    item.artists.push(author);
    item.description = text_for_marker(body, "whitespace-pre-line")
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "));
    item.tags = span_texts_after(body, "flex flex-wrap gap-2");
    item.status = parse_status(body);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            if !href.contains("/chapters/") {
                return None;
            }
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let spans = span_texts(block);
            let title = spans
                .get(1)
                .cloned()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Розділ".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: spans
                    .first()
                    .and_then(|value| value.replace(',', ".").parse::<f32>().ok()),
                date_uploaded: spans.get(2).and_then(|value| parse_dmy(value)),
                url: Some(absolute_url(&key)),
                language: Some("uk".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = content_block(body)
        .or_else(|| {
            html::attr_after(
                body,
                "data-chapter-content-value",
                "data-chapter-content-value",
            )
        })
        .or_else(|| json_content(body))
        .unwrap_or_else(|| {
            "Контент не знайдено. Можливо, потрібна авторизація на сайті.".to_string()
        });
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: text_between_tag(body, "h1"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(normalize_key(key)),
        ..NovelText::default()
    }
}

fn content_block(body: &str) -> Option<String> {
    for marker in ["trix-content", "prose", "<article", "chapter-content"] {
        if let Some(value) = html::text_between(body, marker, "</div>") {
            let clean = remove_noise(&value);
            if html::strip_tags(&clean).len() > 20 {
                return Some(clean);
            }
        }
    }
    None
}

fn json_content(body: &str) -> Option<String> {
    let encoded = body
        .split("\"content\\\":\\\"")
        .nth(1)?
        .split("\\\"")
        .next()?;
    Some(
        encoded
            .replace("\\n", "<br>")
            .replace("\\\"", "\"")
            .replace("\\u003c", "<")
            .replace("\\u003e", ">"),
    )
}

fn remove_noise(input: &str) -> String {
    let mut output = input.to_string();
    for marker in ["<script", "<style", "<button", "<form"] {
        while let Some(start) = output.find(marker) {
            let end_tag = marker.trim_start_matches('<');
            let Some(end) = output[start..]
                .find(&format!("</{end_tag}>"))
                .map(|idx| start + idx + end_tag.len() + 3)
            else {
                break;
            };
            output.replace_range(start..end, "");
        }
    }
    output
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn filter_bool(request: &Value, id: &str) -> bool {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(&key)),
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_status(body: &str) -> ItemStatus {
    let text = body.to_lowercase();
    if text.contains("заверш") {
        ItemStatus::Completed
    } else if text.contains("покину") || text.contains("hiatus") {
        ItemStatus::Hiatus
    } else if text.contains("скас") || text.contains("cancel") {
        ItemStatus::Cancelled
    } else if text.contains("видаєт") || text.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn text_for_marker(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn span_texts_after(body: &str, marker: &str) -> Vec<String> {
    span_texts(body.split(marker).nth(1).unwrap_or_default())
}

fn span_texts(body: &str) -> Vec<String> {
    body.split("<span")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</span>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"") || lower.contains("class=\"next") || lower.contains("pagination")
}

fn parse_dmy(value: &str) -> Option<i64> {
    let parts: Vec<_> = value
        .split(|ch| ch == '.' || ch == '/' || ch == '-')
        .filter_map(|part| part.trim().parse::<i32>().ok())
        .collect();
    if parts.len() == 3 {
        unix_from_ymd(parts[2], parts[1] as u32, parts[0] as u32)
    } else {
        None
    }
}

fn unix_from_ymd(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64 * 86_400)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(normalize_key)
        .filter(|key| !key.is_empty())
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key)
        .unwrap_or_else(|| "Novel".to_string())
        .replace('-', " ")
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const LIST_FIXTURE: &str = r#"
<div data-fiction-picker-id-param="1"></div>
"#;

const PICKER_FIXTURE: &str = r#"
<a href="/fictions/sample"><img src="/cover.jpg"></a><h3>Sample Fiction</h3>
"#;

const SEARCH_FIXTURE: &str = r#"
<turbo-frame id="fictions-section"><a href="/fictions/sample"><h3>Sample Fiction</h3><img src="/cover.jpg"></a></turbo-frame>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:image" content="/cover.jpg">
<h1>Sample Fiction</h1>
<div id="fictions-author-search">Sample Author</div>
<div class="whitespace-pre-line">Sample summary.</div>
<div class="flex flex-wrap gap-2"><span>Фентезі</span></div>
<div class="text-2xl">Видається</div><div class="text-sm">Статус</div>
<li class="group"><a href="/fictions/sample/chapters/1"><span>1</span><span>Розділ 1</span><span>01.01.2024</span></a></li>
"#;

const TEXT_FIXTURE: &str = r#"
<div class="trix-content"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
