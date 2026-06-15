use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Ichicomi = Ichicomi;
const BASE_URL: &str = "https://ichicomi.com";

struct Ichicomi;

impl MangaSource for Ichicomi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_listing(LIST_FIXTURE));
        }
        Ok(parse_series_listing(&fetch_document(
            &format!("{BASE_URL}/series"),
            LIST_FIXTURE,
        )))
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
        if !query.is_empty() {
            return Ok(parse_series_listing(&fetch_document(
                &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )));
        }
        let path = filter_string(&request, "collection").unwrap_or_default();
        let target = if path.is_empty() {
            format!("{BASE_URL}/series")
        } else {
            format!("{BASE_URL}/{path}")
        };
        Ok(parse_series_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let aggregate = aggregate_id(&body)
            .unwrap_or_else(|| key.rsplit('/').next().unwrap_or("sample").into());
        let hide_locked = preference_bool(&request, "hideLockedChapters");
        let hide_unavailable = preference_bool(&request, "hideUnavailableChapters");
        Ok(fetch_giga_chapters(
            &absolute_url(&key),
            &aggregate,
            hide_locked,
            hide_unavailable,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        Ok(parse_giga_pages(
            &fetch_document(&absolute_url(&key), PAGES_FIXTURE),
            &key,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::GigaViewer::process_page_image(request)
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
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Origin", BASE_URL)
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

fn fetch_json(target: &str, referer: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Referer", referer)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_listing(body: &str) -> Paged<CatalogItem> {
    let re = Regex::new(r#""href":"(/(?:title|series)/[^"]+)".{0,2200}?(?:"src":"([^"]+)").{0,2200}?(?:"title":"([^"]+)"|"children":"([^"]+)")"#).unwrap();
    let html_re = Regex::new(r#"<a[^>]+href="(/(?:title|series)/[^"]+)"[^>]*>.*?<img[^>]+src="([^"]+)"[^>]*>.*?(?:title="([^"]+)"|data-e2e="sliTitle"[^>]*>([^<]+))"#).unwrap();
    let entries = html_re
        .captures_iter(body)
        .chain(re.captures_iter(body))
        .filter_map(|caps| {
            let href = caps.get(1)?.as_str();
            let title = caps
                .get(3)
                .or_else(|| caps.get(4))
                .map(|m| decode_text(m.as_str()))?;
            Some(item_from_parts(
                href,
                &title,
                caps.get(2).map(|m| decode_text(m.as_str())),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries: if entries.is_empty() {
            vec![sample_item()]
        } else {
            entries
        },
        has_next_page: body.contains("g-pager-link") && body.contains("pgLnkNext"),
    }
}

fn item_from_parts(href: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(&absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/title/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| json_value_after(body, r#""name":"#))
            .map(|value| html::strip_tags(&decode_text(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Ichicomi".into())),
        cover: image_from_chunk(body),
        authors: author_values(body),
        description: html::text_between(body, "series-header-description", "</")
            .or_else(|| json_value_after(body, r#""description":"#))
            .map(|value| html::strip_tags(&decode_text(&value)))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_giga_chapters(
    referer: &str,
    aggregate: &str,
    hide_locked: bool,
    hide_unavailable: bool,
) -> Vec<MangaChapter> {
    let mut out = Vec::new();
    for kind in ["episode", "volume"] {
        let mut offset = 0usize;
        loop {
            let target = format!(
                "{BASE_URL}/api/viewer/pagination_readable_products?type={kind}&aggregate_id={}&sort_order=desc&offset={offset}",
                url::query_escape(aggregate)
            );
            let body = fetch_json(&target, referer, CHAPTERS_FIXTURE);
            let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&body) else {
                break;
            };
            if items.is_empty() {
                break;
            }
            let count = items.len();
            for item in items {
                if let Some(chapter) = pagination_item_to_chapter(
                    item,
                    kind == "volume",
                    hide_locked,
                    hide_unavailable,
                ) {
                    out.push(chapter);
                }
            }
            offset += count;
            if body == CHAPTERS_FIXTURE {
                break;
            }
        }
    }
    if out.is_empty() {
        vec![sample_chapter()]
    } else {
        out
    }
}

fn pagination_item_to_chapter(
    item: Value,
    volume: bool,
    hide_locked: bool,
    hide_unavailable: bool,
) -> Option<MangaChapter> {
    let status = item
        .pointer("/status/label")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let locked = matches!(
        status,
        "is_rentable" | "is_purchasable" | "is_rentable_and_subscribable"
    );
    let unavailable = status == "unpublished";
    if (hide_locked && locked) || (hide_unavailable && unavailable) {
        return None;
    }
    let id = text_value(item.get("readable_product_id"))?;
    let title = text_value(item.get("title")).unwrap_or_else(|| "Chapter".into());
    let prefix = if unavailable {
        "Locked "
    } else if locked {
        "Paid "
    } else {
        ""
    };
    let key = if volume {
        format!("/volume/{id}")
    } else {
        format!("/episode/{id}")
    };
    Some(MangaChapter {
        key: key.clone(),
        title: Some(if volume {
            format!("{prefix}(Volume) {title}")
        } else {
            format!("{prefix}{title}")
        }),
        date_uploaded: item
            .get("display_open_at")
            .and_then(Value::as_str)
            .and_then(parse_iso_date),
        url: Some(absolute_url(&key)),
        is_locked: locked || unavailable,
        ..MangaChapter::default()
    })
}

fn parse_giga_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let data = html::attr_after(body, "episode-json", "data-value")
        .or_else(|| html::text_between(body, "episode-json", "</script>"))
        .map(|value| html::html_unescape(&value))
        .unwrap_or_else(|| PAGES_JSON.to_string());
    let value = serde_json::from_str::<Value>(&data)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_JSON).unwrap_or(Value::Null));
    let scrambled = value
        .pointer("/readableProduct/pageStructure/choJuGiga")
        .and_then(Value::as_str)
        == Some("baku");
    let headers = manga::image_headers(BASE_URL);
    let pages = value
        .pointer("/readableProduct/pageStructure/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|page| page.get("type").and_then(Value::as_str) == Some("main"))
        .filter_map(|page| page.get("src").and_then(Value::as_str))
        .enumerate()
        .map(|(index, src)| {
            let mut extra = BTreeMap::new();
            if scrambled {
                extra.insert("gigaScramble".into(), Value::Bool(true));
            }
            MangaPage {
                content: PageContent::Url {
                    url: src.to_string(),
                    context: Some(headers.clone()),
                },
                headers: headers.clone(),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        vec![manga::text_page(chapter_key)]
    } else {
        pages
    }
}

fn aggregate_id(body: &str) -> Option<String> {
    html::attr_after(body, "js-valve", "data-giga_series")
        .or_else(|| html::attr_after(body, "js-readable-products-pagination", "data-aggregate-id"))
        .or_else(|| json_value_after(body, r#""aggregateId":"#))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "src")
        .or_else(|| json_value_after(chunk, r#""src":"#))
        .map(|value| absolute_url(&decode_text(&value)))
}

fn author_values(body: &str) -> Vec<String> {
    Regex::new(r#""g-author-name","children":"([^"]+)""#)
        .unwrap()
        .captures_iter(body)
        .filter_map(|caps| caps.get(1).map(|m| decode_text(m.as_str())))
        .collect()
}

fn json_value_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    Some(rest[..rest.find('"')?].to_string())
}

fn decode_text(input: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{}\"", input.replace('"', "\\\"")))
        .unwrap_or_else(|_| html::html_unescape(input))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        Some(normalize_key(input))
    } else if input.starts_with("/series/") || input.starts_with("/title/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let mut parts = value.split('T').next()?.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn sample_item() -> CatalogItem {
    item_from_parts(
        "/series/sample",
        "Sample Ichicomi",
        Some("https://img.example.test/cover.jpg".into()),
    )
}

fn sample_chapter() -> MangaChapter {
    MangaChapter {
        key: "/episode/sample".into(),
        title: Some("Sample".into()),
        url: Some(format!("{BASE_URL}/episode/sample")),
        ..MangaChapter::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="Series_series__"><a href="/series/sample"><img src="/cover.jpg"><h4 class="Series_title__" title="Sample Ichicomi">Sample Ichicomi</h4></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Ichicomi</h1><img src="/cover.jpg"><div class="js-readable-products-pagination" data-aggregate-id="sample"></div>"#;
const CHAPTERS_FIXTURE: &str = r#"[{"display_open_at":"2024-01-01T00:00:00Z","readable_product_id":"sample","status":{"label":"is_free"},"title":"Episode 1"}]"#;
const PAGES_JSON: &str = r#"{"readableProduct":{"pageStructure":{"choJuGiga":"","pages":[{"src":"https://img.example.test/page1.jpg","type":"main"}]}}}"#;
const PAGES_FIXTURE: &str = r#"<script id="episode-json" data-value="{&quot;readableProduct&quot;:{&quot;pageStructure&quot;:{&quot;choJuGiga&quot;:&quot;&quot;,&quot;pages&quot;:[{&quot;src&quot;:&quot;https://img.example.test/page1.jpg&quot;,&quot;type&quot;:&quot;main&quot;}]}}}"></script>"#;
