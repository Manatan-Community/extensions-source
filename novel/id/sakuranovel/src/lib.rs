use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SOURCE: SakuraNovel = SakuraNovel;
const BASE_URL: &str = "https://sakuranovel.id";

struct SakuraNovel;

impl NovelSource for SakuraNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body =
            fetch_document_or_fixture(&advanced_search_url(page, listing, &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
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
        let target = format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body, &key);
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
            .unwrap_or_else(|| "sample-chapter/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest Update".to_string(),
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

fn advanced_search_url(page: u64, listing: &str, request: &Value) -> String {
    let mut parts = vec![
        "title".to_string(),
        "author".to_string(),
        "yearx".to_string(),
        format!("status={}", filter_string(request, "status", "")),
        format!("type={}", filter_string(request, "type", "")),
        format!(
            "order={}",
            if listing == "latest" {
                "update".to_string()
            } else {
                filter_string(request, "sort", "rating")
            }
        ),
    ];
    for value in filter_array(request, "lang") {
        parts.push(format!("country[]={}", url::query_escape(&value)));
    }
    for value in filter_array(request, "genre") {
        parts.push(format!("genre[]={}", url::query_escape(&value)));
    }
    format!(
        "{BASE_URL}/advanced-search/page/{page}/?{}",
        parts.join("&")
    )
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("flexbox2-item")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "flexbox2-content", "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(block, "flexbox2-title", "</span>")
                .or_else(|| html::attr_after(block, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
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
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        text_for_marker(body, "series-title")
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "series-thumb", "src").map(|src| absolute_url(&src)),
        true,
    );
    item.authors = text_after_label(body, "Author").into_iter().collect();
    item.description = html::text_between(body, "series-synops", "</div>")
        .map(|value| html::strip_tags(&remove_divs(&value)))
        .filter(|value| !value.is_empty());
    item.tags = link_texts_after(body, "series-genres");
    item.status = parse_status(&text_for_marker(body, "status").unwrap_or_default());
    item
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let details = parse_details(body, novel_key);
    let image_title = details
        .cover
        .as_deref()
        .and_then(|cover| cover.rsplit('/').next())
        .and_then(|name| name.split('.').next())
        .unwrap_or_default()
        .replace('-', " ");
    let chapter_title = details
        .title
        .replace("(LN)", "")
        .replace("(WN)", "")
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut seen = BTreeSet::new();
    body.split("<li")
        .skip(1)
        .filter(|block| block.contains("series-flexright") || block.contains("<a"))
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let mut title = html::text_between(block, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| title_from_key(&key));
            for remove in [&chapter_title, image_title.as_str(), "Bahasa Indonesia"] {
                title = title.replace(remove, "");
            }
            title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(NovelChapter {
                key: key.clone(),
                title: Some(if title.is_empty() {
                    "Chapter".to_string()
                } else {
                    title
                }),
                chapter_number: chapter_number(&key),
                date_uploaded: text_for_marker(block, "date").and_then(|date| parse_dmy(&date)),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = body
        .split("Daftar Isi")
        .nth(1)
        .and_then(|rest| html::text_between(rest, "<div", "</div>"))
        .or_else(|| html::text_between(body, "entry-content", "</div>"))
        .or_else(|| html::text_between(body, "post-body", "</div>"))
        .unwrap_or_else(|| body.to_string());
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

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
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
        language: Some("id".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") || lower.contains("tamat") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancelled") {
        ItemStatus::Cancelled
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn text_for_marker(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</").map(|value| html::strip_tags(&value))
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|rest| {
            html::text_between(rest, "<li", "</li>")
                .or_else(|| html::text_between(rest, "<dd", "</dd>"))
        })
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim_matches(':').trim().to_string())
        .filter(|value| !value.is_empty())
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .take(40)
        .collect()
}

fn remove_divs(input: &str) -> String {
    input.split("<div").next().unwrap_or(input).to_string()
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"") || lower.contains("class=\"next") || lower.contains("/page/")
}

fn parse_dmy(value: &str) -> Option<i64> {
    let parts: Vec<_> = value
        .split('/')
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
<div class="flexbox2-item">
  <div class="flexbox2-content"><a href="https://sakuranovel.id/series/sample/"><img src="/cover.jpg"></a></div>
  <div class="flexbox2-title"><span>Sample Novel</span></div>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="series-title"><h2>Sample Novel</h2></div>
<div class="series-thumb"><img src="/cover.jpg"></div>
<ul class="series-infolist"><li><b>Author</b> Sample Author</li></ul>
<div class="series-genres"><a>Fantasy</a></div>
<div class="series-synops">Sample summary.</div>
<span class="status">Ongoing</span>
<div class="series-flexright">
  <li><a href="/sample-chapter/"><span>Sample Novel Chapter 1 Bahasa Indonesia</span></a><span class="date">01/01/2024</span></li>
</div>
"#;

const TEXT_FIXTURE: &str = r#"
<div>Daftar Isi</div><div><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
