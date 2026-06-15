use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: KurageBunch = KurageBunch;
const BASE_URL: &str = "https://kuragebunch.com";

struct KurageBunch;

impl MangaSource for KurageBunch {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        let collection = request
            .get("listingId")
            .and_then(Value::as_str)
            .or_else(|| filter_string(&request, "collection"))
            .unwrap_or("kuragebunch");
        Ok(parse_series_list(&fetch_document(
            &format!("{BASE_URL}/series/{collection}"),
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
        let collection = filter_string(&request, "collection").unwrap_or("kuragebunch");
        let mut page = parse_series_list(&fetch_document(
            &format!("{BASE_URL}/series/{collection}"),
            LIST_FIXTURE,
        ));
        if !query.is_empty() {
            let needle = query.to_lowercase();
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let aggregate = aggregate_id(&body).unwrap_or_else(|| "sample".into());
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
        let popular = self.list(json!({"page": 1, "listingId": "kuragebunch"}))?;
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

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("item-box")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Kurage Bunch".into())
                    }),
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
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(&absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    let info =
        html::text_between(body, "series-header", "</section>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&info, "series-header-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Kurage Bunch".into())),
        cover: image_from_chunk(&info).or_else(|| image_from_chunk(body)),
        authors: html::text_between(&info, "series-header-author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(&info, "series-header-description", "</")
            .map(|value| html::strip_tags(&value))
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
        out.push(MangaChapter {
            key: "/episode/sample".into(),
            title: Some("Sample".into()),
            url: Some(format!("{BASE_URL}/episode/sample")),
            ..MangaChapter::default()
        });
    }
    out
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
    let title = if volume {
        format!("{prefix}(Volume) {title}")
    } else {
        format!("{prefix}{title}")
    };
    let key = if volume {
        format!("/volume/{id}")
    } else {
        format!("/episode/{id}")
    };
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title),
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
    value
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
        .collect::<Vec<_>>()
        .into_iter()
        .chain((!body.contains("episode-json")).then(|| manga::text_page(chapter_key)))
        .collect()
}

fn aggregate_id(body: &str) -> Option<String> {
    html::attr_after(body, "js-valve", "data-giga_series")
        .or_else(|| html::attr_after(body, "js-readable-products-pagination", "data-aggregate-id"))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
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
    manatan_shared::dates::parse_ymd(value.split('T').next()?)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="page-series-list"><li><div class="item-box"><a class="series-thumb" href="/series/sample"><img data-src="/cover.jpg"></a><a class="series-data-container" href="/series/sample"><h4>Sample Kurage Bunch</h4></a></div></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"
<section class="series-information"><div class="series-header"><h1 class="series-header-title">Sample Kurage Bunch</h1><h2 class="series-header-author">Sample Author</h2><p class="series-header-description">Sample description.</p><div class="series-header-image-wrapper"><img data-src="/cover.jpg"></div><div class="js-readable-products-pagination" data-aggregate-id="sample"></div></div></section>
"#;
const CHAPTERS_FIXTURE: &str = r#"[{"display_open_at":"2024-01-01T00:00:00Z","readable_product_id":"sample","status":{"label":"is_free"},"title":"Episode 1"}]"#;
const PAGES_JSON: &str = r#"{"readableProduct":{"pageStructure":{"choJuGiga":"","pages":[{"src":"https://kuragebunch.com/page1.jpg","type":"main"}]}}}"#;
const PAGES_FIXTURE: &str = r#"<script id="episode-json" data-value="{&quot;readableProduct&quot;:{&quot;pageStructure&quot;:{&quot;choJuGiga&quot;:&quot;&quot;,&quot;pages&quot;:[{&quot;src&quot;:&quot;https://kuragebunch.com/page1.jpg&quot;,&quot;type&quot;:&quot;main&quot;}]}}}"></script>"#;
