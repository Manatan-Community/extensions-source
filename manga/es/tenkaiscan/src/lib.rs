use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: FalcoScan = FalcoScan;
const BASE_URL: &str = "https://falcoscan.net";
const NAME: &str = "Falco Scan";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct FalcoScan;

impl MangaSource for FalcoScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_popular(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let latest = listing_id(&request) == "latest";
        let target = if latest {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/ranking")
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: if latest {
                parse_latest(&body)
            } else {
                parse_popular(&body)
            },
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_search(&fetch_document_or_fixture(
                &search_url(query, request.get("filters").unwrap_or(&Value::Null)),
                SEARCH_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample/1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
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
        if input.starts_with(BASE_URL) && (input.contains("/comic/") || input.contains("/comics/"))
        {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
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

fn search_url(query: &str, filters: &Value) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("search={}", url::query_escape(query)));
    } else if let Some(value) = selected_filter(filters, "filter") {
        params.push(format!("filter={}", url::query_escape(&value)));
    } else if let Some(value) = selected_filter(filters, "gen") {
        params.push(format!("gen={}", url::query_escape(&value)));
    } else if let Some(value) = selected_filter(filters, "status") {
        params.push(format!("status={}", url::query_escape(&value)));
    }
    if params.is_empty() {
        format!("{BASE_URL}/comics")
    } else {
        format!("{BASE_URL}/comics?{}", params.join("&"))
    }
}

fn selected_filter(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("card")
        .skip(1)
        .filter(|chunk| chunk.contains("window.location.href") || chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = onclick_href(chunk).or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = class_text(chunk, "name")
                .or_else(|| class_text(chunk, "content"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| NAME.to_string());
            Some(catalog_item(&href, title, image_from_chunk(chunk), false))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("content") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = class_text(chunk, "content")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| NAME.to_string());
            Some(catalog_item(&href, title, image_from_chunk(chunk), false))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    let entries = parse_latest(body);
    if entries.is_empty() {
        parse_popular(body)
    } else {
        entries
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
        .map(|value| normalize_key(&value))
        .unwrap_or_else(|| "/comic/sample".to_string());
    let details = html::text_between(body, "text-details", "</div>").unwrap_or_else(|| body.into());
    CatalogItem {
        key: key.clone(),
        title: class_text(&details, "name-rating")
            .or_else(|| {
                html::text_between(body, "<title", "</title>").map(|v| html::strip_tags(&v))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| NAME.to_string()),
        cover: html::attr_after(body, "img-details", "data-src")
            .or_else(|| html::attr_after(body, "img-details", "src"))
            .or_else(|| image_from_chunk(body))
            .map(|value| absolute_url(&value)),
        description: text_after_label(body, "Sinopsis")
            .or_else(|| class_text(body, "sec"))
            .filter(|value| !value.is_empty()),
        authors: label_values(body, "Autor"),
        artists: label_values(body, "Artista"),
        tags: label_values(body, "Generos"),
        status: label_values(body, "Status")
            .first()
            .map(|value| parse_status(value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("card-caps")
        .skip(1)
        .filter_map(|chunk| {
            let href = onclick_href(chunk).or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = class_text(chunk, "color-white")
                .or_else(|| class_text(chunk, "text-cap"))
                .unwrap_or_else(|| "Capitulo".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: class_text(chunk, "color-medium-gray")
                    .and_then(|value| parse_dmy(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("img-blade") || chunk.contains("data-src") || chunk.contains("src=")
        })
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_item(
    href: &str,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: html::strip_tags(&title),
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
}

fn onclick_href(chunk: &str) -> Option<String> {
    let onclick = html::attr(chunk, "onclick")?;
    onclick
        .split("window.location.href=")
        .nth(1)
        .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
        .filter(|value| !value.is_empty())
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    html::class_blocks(body, class_name)
        .filter_map(|chunk| {
            let rest = chunk.split_once('>')?.1;
            rest.split("</div>")
                .next()
                .or_else(|| rest.split("</span>").next())
                .or_else(|| rest.split("</h4>").next())
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
        })
        .next()
}

fn label_values(body: &str, label: &str) -> Vec<String> {
    text_after_label(body, label)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let start = body
        .find(&format!("contains({label})"))
        .or_else(|| body.find(label))?;
    let chunk = &body[start..body.len().min(start + 700)];
    if let Some(text) = html::text_between(chunk, "<p", "</p>") {
        let cleaned = html::strip_tags(&text);
        let cleaned = cleaned
            .trim_start_matches(label)
            .trim_start_matches(':')
            .trim()
            .to_string();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    html::text_between(chunk, "</span>", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "en emision" | "en emisión" | "ongoing" => ItemStatus::Ongoing,
        "finalizado" | "completed" => ItemStatus::Completed,
        "cancelado" | "canceled" => ItemStatus::Cancelled,
        "en espera" | "hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_dmy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

fn normalize_key(input: &str) -> String {
    let value = input.trim().trim_end_matches('/');
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

const LIST_FIXTURE: &str = r#"
<section class="trending"><div class="row">
<div class="card" onclick="window.location.href='/comic/sample'"><img src="/cover.jpg"><div class="name"><h4 class="color-white">Sample</h4></div></div>
<a href="/comic/latest"><img data-src="/latest.jpg"><div class="content"><h4 class="color-white">Latest</h4></div></a>
</div></section>
"#;

const SEARCH_FIXTURE: &str = r#"
<section class="trending"><div class="row"><div class="col-xxl-9"><div class="row">
<div><a href="/comic/sample"><img src="/cover.jpg"><div class="content"><h4 class="color-white">Sample</h4></div></a></div>
</div></div></div></section>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="page-content"><div class="text-details">
<img class="img-details" src="/cover.jpg">
<div class="name-rating">Sample</div>
<p class="sec">Summary</p>
<div class="soft-details">
<p><span>Autor</span> Author</p>
<p><span>Artista</span> Artist</p>
<p><span>Status</span> En emision</p>
<p><span>Generos</span> Drama, Romance</p>
</div></div>
<div class="card-caps" onclick="window.location.href='/comic/sample/1'"><div class="text-cap"><span class="color-white">Capitulo 1</span><span class="color-medium-gray">01/01/2024</span></div></div>
</div>
"#;

const PAGES_FIXTURE: &str =
    r#"<div class="page-content"><div class="img-blade"><img src="/page1.jpg"></div></div>"#;

export_manga_source!(SOURCE);
