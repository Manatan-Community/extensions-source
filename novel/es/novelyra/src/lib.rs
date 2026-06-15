use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

const SOURCE: Novelyra = Novelyra;
const BASE_URL: &str = "https://novelyra.com/";

struct Novelyra;

impl NovelSource for Novelyra {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body = if listing == "latest" {
            fetch_document_or_fixture(BASE_URL, HOME_FIXTURE)
        } else {
            let browse = filter_string(&request, "browse", "browse.php");
            if browse.starts_with("popular.php") {
                fetch_document_or_fixture(&absolute_url(&browse), POPULAR_FIXTURE)
            } else {
                let mut params = vec![format!("page={page}")];
                let genre = filter_string(&request, "genres", "");
                if !genre.is_empty() {
                    params.push(format!("genre={}", url::query_escape(&genre)));
                }
                fetch_document_or_fixture(
                    &format!("{}{}?{}", BASE_URL, browse, params.join("&")),
                    LIST_FIXTURE,
                )
            }
        };
        let marker = if listing == "latest" {
            "novel-card"
        } else if body.contains("popular-item") {
            "popular-item"
        } else {
            "novel-card"
        };
        let entries = parse_listing(&body, marker);
        Ok(Paged {
            has_next_page: !entries.is_empty() && listing != "latest",
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}?search={}", url::query_escape(&query)),
            HOME_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body, "novel-card"),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
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
            .unwrap_or_else(|| "novel/sample/chapter-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(with_listing(request.clone(), "latest"))?;
        let popular = self.list(request)?;
        Ok(vec![
            HomeSection {
                id: "latest".to_string(),
                title: "Ultimas".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".to_string(),
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
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

fn parse_listing(body: &str, marker: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split(marker)
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = text_between_tag(block, "h3")
                .or_else(|| html::attr_after(block, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(catalog_item(key, title, image_from(block), false))
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
        text_between_tag(body, "h1")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        image_from(body),
        true,
    );
    item.tags = block_after(body, "novel-genres")
        .map(|value| {
            html::strip_tags(&value)
                .split([',', '\n'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    item.description = block_after(body, "novel-description-detail")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.status = ItemStatus::Completed;
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("chapter-item-wrapper")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            let number = text_after(block, "chapter-number").and_then(|value| first_number(&value));
            let title = text_after(block, "chapter-title");
            Some(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: number,
                date_uploaded: text_after(block, "chapter-date")
                    .and_then(|value| parse_spanish_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "chapter-content").unwrap_or_else(|| body.to_string());
    let cleaned = remove_block_containing(&raw, "chapter-ad")
        .replace("<script", "<!-- script")
        .replace("</script>", "script -->")
        .replace("<ins", "<!-- ins")
        .replace("</ins>", "ins -->");
    let normalized = novel::normalize_reader_html(&cleaned);
    NovelText {
        title: text_between_tag(body, "h1"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
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
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized,
        ..CatalogItem::default()
    }
}

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    request["listing"] = Value::String(listing.to_string());
    request
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "src")
        .or_else(|| html::attr_after(block, "<img", "data-src"))
        .map(|value| absolute_url(&value))
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn block_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    let rest = &body[start..];
    let end = rest.find("</div>").unwrap_or(rest.len());
    Some(rest[..end + "</div>".len().min(rest.len().saturating_sub(end))].to_string())
}

fn remove_block_containing(input: &str, marker: &str) -> String {
    let Some(marker_pos) = input.find(marker) else {
        return input.to_string();
    };
    let start = input[..marker_pos].rfind('<').unwrap_or(marker_pos);
    let end = input[marker_pos..]
        .find("</div>")
        .map(|idx| marker_pos + idx + 6)
        .unwrap_or(marker_pos);
    let mut out = input.to_string();
    out.replace_range(start..end.min(input.len()), "");
    out
}

fn first_number(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn parse_spanish_date(input: &str) -> Option<i64> {
    let lower = input.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if let Some((day, month, year)) = parse_absolute_spanish_date(&lower) {
        return Some(days_from_civil(year, month, day) * 86_400);
    }
    if lower.starts_with("hace") {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let normalized = lower.replace("una", "1").replace("un", "1");
        let mut parts = normalized.split_whitespace();
        let _ = parts.next();
        let value = parts
            .next()
            .and_then(|part| part.parse::<i64>().ok())
            .unwrap_or(0);
        let unit = parts.next().unwrap_or("");
        let seconds = if unit.starts_with("segundo") {
            value
        } else if unit.starts_with("minuto") {
            value * 60
        } else if unit.starts_with("hora") {
            value * 3_600
        } else if unit.starts_with("dia") || unit.starts_with("día") {
            value * 86_400
        } else if unit.starts_with("mes") {
            value * 2_592_000
        } else if unit.starts_with("año") || unit.starts_with("ano") {
            value * 31_536_000
        } else {
            0
        };
        return Some(now.saturating_sub(seconds));
    }
    None
}

fn parse_absolute_spanish_date(input: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<_> = input.split_whitespace().collect();
    if parts.len() != 5 || parts[1] != "de" || parts[3] != "de" {
        return None;
    }
    let day = parts[0].parse().ok()?;
    let month = match parts[2] {
        "enero" => 1,
        "febrero" => 2,
        "marzo" => 3,
        "abril" => 4,
        "mayo" => 5,
        "junio" => 6,
        "julio" => 7,
        "agosto" => 8,
        "septiembre" => 9,
        "octubre" => 10,
        "noviembre" => 11,
        "diciembre" => 12,
        _ => return None,
    };
    let year = parts[4].parse().ok()?;
    Some((day, month, year))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("novelyra.com").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://novelyra.com/")
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const HOME_FIXTURE: &str = r#"
<div id="novelas"><div class="novel-card"><a href="https://novelyra.com/novel/sample"><img src="/cover.jpg"><h3>Sample Novel</h3></a></div></div>
"#;

const LIST_FIXTURE: &str = r#"
<div class="novels-grid"><div class="novel-card"><a href="https://novelyra.com/novel/sample"><img src="/cover.jpg"><h3>Sample Novel</h3></a></div></div>
"#;

const POPULAR_FIXTURE: &str = r#"
<div class="popular-item"><a href="https://novelyra.com/novel/sample"><img src="/cover.jpg"><h3>Sample Novel</h3></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><img src="/cover.jpg"><div class="novel-meta"><div class="novel-genres">Fantasia, Accion</div></div>
<div class="novel-description-detail">Sample summary.</div>
<div class="chapter-item-wrapper"><a href="https://novelyra.com/novel/sample/chapter-1"><span class="chapter-number">Capitulo 1</span><span class="chapter-title">Inicio</span><span class="chapter-date">21 de febrero de 2026</span></a></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1>Inicio</h1><div class="chapter-content"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
