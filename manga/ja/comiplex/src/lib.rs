use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionError, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Comiplex = Comiplex;
const BASE_URL: &str = "https://viewer.heros-web.com";
const SOURCE_NAME: &str = "Comiplex";
const DEFAULT_COLLECTION: &str = "heros";

struct Comiplex;

impl MangaSource for Comiplex {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        parse_series_list_result(&fetch_document(&format!(
            "{BASE_URL}/series/{DEFAULT_COLLECTION}"
        ))?)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            let collection = filter_string(&request, "collection").unwrap_or(DEFAULT_COLLECTION);
            let target = if collection.is_empty() {
                format!("{BASE_URL}/series")
            } else {
                format!("{BASE_URL}/series/{collection}")
            };
            return parse_series_list_result(&fetch_document(&target)?);
        }
        let page = page(&request);
        let target = if page > 1 {
            format!(
                "{BASE_URL}/search?q={}&page={page}",
                url::query_escape(query)
            )
        } else {
            format!("{BASE_URL}/search?q={}", url::query_escape(query))
        };
        parse_series_list_result(&fetch_document(&target)?)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&absolute_url(&key))?;
        let aggregate =
            aggregate_id(&body).ok_or_else(|| err("could not find GigaViewer aggregate id"))?;
        let hide_locked = preference_bool(&request, "hide_locked")
            || preference_bool(&request, "hideLockedChapters");
        let hide_unavailable = preference_bool(&request, "hide_unavailable")
            || preference_bool(&request, "hideUnavailableChapters");
        Ok(fetch_giga_chapters(
            &absolute_url(&key),
            &aggregate,
            hide_locked,
            hide_unavailable,
        )?)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        parse_giga_pages_result(&fetch_document(&absolute_url(&key))?)
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
                item: Some(details_by_key(&key)?),
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

fn fetch_document(target: &str) -> ExtensionResult<String> {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .map_err(|error| err(&format!("fetch failed for {target}: {}", error.message)))
}

fn fetch_json(target: &str, referer: &str) -> ExtensionResult<String> {
    client()
        .get(target)
        .header("Referer", referer)
        .xhr()
        .send_text()
        .map_err(|error| err(&format!("fetch failed for {target}: {}", error.message)))
}

fn parse_series_list_result(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let page = parse_series_list(body);
    if page.entries.is_empty() {
        Err(err("no manga entries found in GigaViewer listing"))
    } else {
        Ok(page)
    }
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .chain(body.split("<a").skip(1))
        .filter(|chunk| chunk.contains("/series/") || chunk.contains("/title/"))
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            if !key.starts_with("/series/") && !key.starts_with("/title/") {
                return None;
            }
            let title = html::attr_after(chunk, "<a", "data-series-name")
                .or_else(|| html::attr(chunk, "data-series-name"))
                .or_else(|| html::text_between(chunk, "SearchResultItem_series_title", "</"))
                .or_else(|| html::text_between(chunk, "series-title", "</"))
                .or_else(|| html::text_between(chunk, "series_title", "</"))
                .or_else(|| html::text_between(chunk, "item-series-title", "</"))
                .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::text_between(chunk, "<p", "</p>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| SOURCE_NAME.into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pager-next") || body.contains("rel=\"next\""),
    }
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let body = fetch_document(&absolute_url(key))?;
    Ok(parse_details(&body, key))
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info =
        html::text_between(body, "series-header", "</section>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&info, "series-header-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SOURCE_NAME.into())),
        cover: image_from_chunk(&info).or_else(|| image_from_chunk(body)),
        authors: html::text_between(&info, "series-header-author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(&info, "series-header-description", "</")
            .or_else(|| html::attr_after(body, "name=\"description\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(key)),
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
) -> ExtensionResult<Vec<MangaChapter>> {
    let mut out = Vec::new();
    for kind in ["episode", "volume"] {
        let mut offset = 0usize;
        loop {
            let target = format!(
                "{BASE_URL}/api/viewer/pagination_readable_products?type={kind}&aggregate_id={}&sort_order=desc&offset={offset}",
                url::query_escape(aggregate)
            );
            let body = fetch_json(&target, referer)?;
            let items = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.as_array().cloned())
                .ok_or_else(|| err("GigaViewer chapter API returned unexpected JSON"))?;
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
        }
    }
    if out.is_empty() {
        Err(err("no readable products found for this series"))
    } else {
        Ok(out)
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

fn parse_giga_pages_result(body: &str) -> ExtensionResult<Vec<MangaPage>> {
    let data = html::attr_after(body, "episode-json", "data-value")
        .or_else(|| html::text_between(body, "episode-json", "</script>"))
        .map(|value| html::html_unescape(&value))
        .ok_or_else(|| err("could not find embedded GigaViewer episode JSON"))?;
    let value = serde_json::from_str::<Value>(&data)
        .map_err(|error| err(&format!("invalid GigaViewer page JSON: {error}")))?;
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
        Err(err("no image pages found"))
    } else {
        Ok(pages)
    }
}

fn aggregate_id(body: &str) -> Option<String> {
    html::attr_after(body, "js-valve", "data-giga_series")
        .or_else(|| html::attr_after(body, "js-readable-products-pagination", "data-aggregate-id"))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr_after(chunk, "<source", "srcset"))
        .map(|value| {
            value
                .split_whitespace()
                .next()
                .unwrap_or(&value)
                .to_string()
        })
        .map(|value| absolute_url(&value))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/series/") || input.starts_with("/title/")
    {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
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
    Some(days_from_civil(year, month, day) * 86_400_000)
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<ul class="series-items"><li class="series-item"><a href="/series/sample"><div class="series-item-thumb"><img data-src="/cover.jpg"></div><h4 class="item-series-title">Sample Comiplex</h4></a></li></ul>
"#;
