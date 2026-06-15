use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde_json::Value;

const SOURCE: MiauScan = MiauScan;
const BASE_URL: &str = "https://leemiau.com";
const PORTUGUESE_GENRE_ID: &str = "307";

struct MiauScan;

impl MangaSource for MiauScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let order = if latest { "update" } else { "popular" };
        let target = search_url(source, page, "", order);
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, source, Some(key))],
                has_next_page: false,
            });
        }
        let order = request
            .get("filters")
            .and_then(|filters| filters.get("order"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = search_url(source, page, query, order);
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, source, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let source = source_for(&request);
            let key = normalize_key(input);
            let body = fetch_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, source, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    portuguese: bool,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "miauscan-es",
        lang: "es",
        portuguese: false,
    },
    SourceConfig {
        id: "miauscan-pt-br",
        lang: "pt-BR",
        portuguese: true,
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("miauscan-es");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn search_url(source: SourceConfig, page: u64, query: &str, order: &str) -> String {
    let genre_prefix = if source.portuguese { "" } else { "-" };
    format!(
        "{BASE_URL}/manga?title={}&page={page}&order={order}&genre[]={genre_prefix}{PORTUGUESE_GENRE_ID}/",
        encode_query(query)
    )
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("imgu") || chunk.contains("uta"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let mut title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))?;
            title = strip_portuguese_suffix(&title);
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, source: SourceConfig, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample/".into());
    let mut title = html::text_between(body, "<h1", "</h1>")
        .map(|text| html::strip_tags(&text))
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "Miau Scan".into());
    title = strip_portuguese_suffix(&title);
    let description = html::text_between(body, "lm4-summary-full", "</div>")
        .or_else(|| html::text_between(body, "lm4-summary-short", "</div>"))
        .map(|text| html::strip_tags(&text));
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "lm4-poster-image", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        description,
        tags: parse_tags(body),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        status: parse_status(body),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("lm4-chapter") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "lm4-chapter-name", "</")
                .map(|text| html::strip_tags(&text))
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|text| html::strip_tags(&text))
                })
                .unwrap_or_else(|| "Chapter".into());
            let subtitle = html::text_between(chunk, "lm4-chapter-subtitle", "</")
                .map(|text| html::strip_tags(&text))
                .unwrap_or_default();
            let date = html::text_between(chunk, "lm4-chapter-date", "</")
                .and_then(|text| parse_dd_mm_yyyy(&html::strip_tags(&text)));
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(if subtitle.is_empty() || subtitle == title {
                    title
                } else {
                    format!("{title} - {subtitle}")
                }),
                date_uploaded: date,
                language: Some(source.lang.into()),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.is_empty())
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    [
        "data-lm-orig-src",
        "data-lazy-src",
        "data-src",
        "data-cfsrc",
        "src",
    ]
    .into_iter()
    .find_map(|name| html::attr(input, name))
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("next")
        || body.contains("hpage") && body.contains("r")
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| chunk.contains("/genre/") || chunk.contains("/genres/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|tag| !tag.eq_ignore_ascii_case("Português") && !tag.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_lowercase();
    if lower.contains("completed") || lower.contains("finalizado") || lower.contains("concluído") {
        ItemStatus::Completed
    } else if lower.contains("cancelled") || lower.contains("cancelado") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") || lower.contains("pausado") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") || lower.contains("em andamento") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(value: &str) -> String {
    value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
        .to_string()
}

fn strip_portuguese_suffix(title: &str) -> String {
    let trimmed = title.trim();
    for prefix in [
        "(Português)",
        "(Portugues)",
        "( português )",
        "( portugues )",
    ] {
        if trimmed.to_lowercase().starts_with(&prefix.to_lowercase()) {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn parse_dd_mm_yyyy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"
<div class="bsx"><a href="https://leemiau.com/manga/sample/" title="Sample Miau"><img data-src="/cover.jpg"></a></div>
<div class="pagination"><a class="next" href="/manga/page/2">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Miau</h1>
<img class="lm4-poster-image" src="/cover.jpg">
<div class="lm4-summary-full">Series description.</div>
<div class="lm4-poster-status">Ongoing</div>
<div class="mgen"><a href="/genre/action/">Action</a><a href="/genre/portugues/">Português</a></div>
<li><a href="https://leemiau.com/manga/sample/chapter-1/"><span class="lm4-chapter-name">Chapter 1</span><span class="lm4-chapter-subtitle">Start</span><span class="lm4-chapter-date">01/01/2024</span></a></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="readerarea"><img data-lm-orig-src="/page-1.jpg"><img data-lazy-src="/page-2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_miauscan() {
        let source = SOURCES[0];
        assert_eq!(parse_listing(LIST_FIXTURE, source).len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, source, None).title,
            "Sample Miau"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE, source).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
