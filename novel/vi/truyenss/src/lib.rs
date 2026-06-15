use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: TruyenSs = TruyenSs;
const BASE_URL: &str = "https://truyenss.com";

struct TruyenSs;

impl NovelSource for TruyenSs {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str);
        let target = if listing == Some("latest") {
            format!("{BASE_URL}/")
        } else {
            let genre = lnreader::filter_string(&request, "genre", "tien-hiep");
            if page <= 1 {
                format!("{BASE_URL}/{genre}")
            } else {
                format!("{BASE_URL}/{genre}?page={page}")
            }
        };
        let entries = parse_listing(&fetch(&target, LIST_FIXTURE));
        Ok(Paged {
            has_next_page: listing != Some("latest") && !entries.is_empty(),
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page <= 1 {
            format!("{BASE_URL}/tim-kiem?q={}", url::query_escape(query))
        } else {
            format!(
                "{BASE_URL}/tim-kiem?q={}&page={page}",
                url::query_escape(query)
            )
        };
        let entries = parse_listing(&fetch(&target, LIST_FIXTURE));
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "truyen/ornek".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "truyen/ornek".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
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
            .unwrap_or_else(|| "truyen/ornek/chuong-1".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        let raw = largest_div_html(&body).unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(text_response(&key, &raw))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Truyen", popular),
            section("latest", "Moi cap nhat", latest),
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

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for part in body.split("<a").skip(1) {
        let Some(href) = html::attr(part, "href") else {
            continue;
        };
        let key = normalize_key(&href);
        if !key.starts_with("truyen/") || key.split('/').count() != 2 || seen.contains(&key) {
            continue;
        }
        let title = html::attr(part, "title")
            .or_else(|| text_between(part, ">", "</a>"))
            .unwrap_or_else(|| "TruyenSS".to_string());
        seen.push(key.clone());
        out.push(CatalogItem {
            key: key.clone(),
            title,
            cover: attr_after(part, "<img", "data-src")
                .or_else(|| attr_after(part, "<img", "src"))
                .map(|v| absolute_url(&v)),
            url: Some(absolute_url(&key)),
            language: Some("vi".to_string()),
            ..CatalogItem::default()
        });
    }
    out
}

fn fetch_details(key: &str) -> CatalogItem {
    parse_details(&fetch(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = html::text_between(body, "info_truyen", "</div>").unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "<h1", "</h1>").unwrap_or_else(|| "TruyenSS".to_string()),
        cover: attr_after(body, "info_truyen", "src").map(|v| absolute_url(&v)),
        description: html::text_between(body, "line-height-3", "</div>")
            .or_else(|| html::text_between(body, "gioithieu", "</div>"))
            .map(|v| html::strip_tags(&v)),
        authors: field_after(&info, "Tac Gia:")
            .or_else(|| field_after(&info, "Tác Giả:"))
            .into_iter()
            .collect(),
        tags: body
            .split("badge")
            .filter_map(|part| text_between(part, ">", "</a>"))
            .collect(),
        status: status_from_text(&info),
        url: Some(absolute_url(key)),
        language: Some("vi".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    for part in body.split("href=\"#").skip(1) {
        let Some(end) = part.find('"') else { continue };
        let num = part[..end].parse::<u32>().ok();
        let Some(number) = num else { continue };
        let title = text_between(part, ">", "</a>").unwrap_or_else(|| format!("Chuong {number}"));
        chapters.push(NovelChapter {
            key: format!("{novel_key}/chuong-{number}"),
            title: Some(title),
            chapter_number: Some(number as f32),
            url: Some(absolute_url(&format!("{novel_key}/chuong-{number}"))),
            language: Some("vi".to_string()),
            ..NovelChapter::default()
        });
    }
    chapters
}

fn largest_div_html(body: &str) -> Option<String> {
    let mut best = None;
    let mut best_p = 0usize;
    for part in body.split("<div").skip(1) {
        let content = html::text_between(part, ">", "</div>").unwrap_or_default();
        let count = content.matches("<p").count();
        if count > best_p {
            best_p = count;
            best = Some(content);
        }
    }
    best.filter(|_| best_p >= 1)
}

fn text_response(key: &str, raw: &str) -> NovelText {
    let normalized = novel::normalize_reader_html(raw);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn status_from_text(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoan") || lower.contains("hoàn") || lower.contains("full") {
        ItemStatus::Completed
    } else if lower.contains("dang") || lower.contains("đang") || lower.contains("ra chuong") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn field_after(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)?;
    html::strip_tags(&text[start + marker.len()..])
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn absolute_url(key: &str) -> String {
    lnreader::absolute_url(BASE_URL, key)
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    lnreader::key_from_url(BASE_URL, input)
}

fn attr_after(input: &str, marker: &str, attr: &str) -> Option<String> {
    html::attr_after(input, marker, attr).filter(|value| !value.trim().is_empty())
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    if start == ">" {
        let idx = input.find('>')?;
        let rest = &input[idx + 1..];
        let end_idx = rest.find(end)?;
        return Some(html::strip_tags(&rest[..end_idx]));
    }
    html::text_between(input, start, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

const LIST_FIXTURE: &str =
    r#"<a href="/truyen/ornek" title="Truyen mau"><img src="/images/no_avatar.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r##"<h1>Truyen mau</h1><div class="info_truyen">Tac Gia: Tac gia<br>Tinh Trang: Dang ra</div><a href="#1">Chuong 1</a>"##;
const TEXT_FIXTURE: &str = r#"<div><p>Noi dung mau.</p></div>"#;

export_novel_source!(SOURCE);
