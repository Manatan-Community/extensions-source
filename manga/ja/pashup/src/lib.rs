use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    ProcessedImage, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{
    dates, html, manga, manga_image,
    sdk::http::HttpClient,
    speedbinb::SpeedBinbReader,
    url,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: PashUp = PashUp;
const BASE_URL: &str = "https://pash-up.jp";
const API_URL: &str = "https://pash-up.jp/pageapi";
const PAGE_LIMIT: u64 = 10;

struct PashUp;

impl MangaSource for PashUp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_entries(&fetch_json(&format!(
                "{API_URL}/products.php?type=update&period=daily&category=2&unit=2&lastest=1&limit={PAGE_LIMIT}&offset={}&_={}",
                (page - 1) * PAGE_LIMIT,
                timestamp_ms()
            ), LATEST_FIXTURE)))
        } else {
            Ok(parse_entries(&fetch_json(&format!(
                "{API_URL}/contents.php?type=ranking&period=daily&category=2&limit={PAGE_LIMIT}&offset={}&_={}",
                (page - 1) * PAGE_LIMIT,
                timestamp_ms()
            ), LIST_FIXTURE)))
        }
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
        let body = fetch_json(
            &format!(
                "{API_URL}/contents.php?type=search&reserve=1&keyword={}&limit=9999&_={}",
                url::query_escape(query),
                timestamp_ms()
            ),
            LIST_FIXTURE,
        );
        let mut page = parse_entries(&body);
        page.entries.retain(|item| item.extra.get("pashCategory").and_then(Value::as_str) == Some("2"));
        page.has_next_page = false;
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(parse_chapters(&chapter_list_body(&key), hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample#sample-product".into());
        let (series_id, product_id) = key.split_once('#').unwrap_or((key.as_str(), ""));
        let Some(product) = collect_products(&chapter_list_body(series_id))
            .into_iter()
            .find(|product| product.id == product_id)
        else {
            return Ok(Vec::new());
        };
        if product.download_url.contains("/pageapi/download") {
            return Ok(Vec::new());
        }
        let Some(cid) = query_param(&product.download_url, "cid") else {
            return Ok(Vec::new());
        };
        let cphp = fetch_json(
            &format!("{API_URL}/viewer/c.php?cid={}", url::query_escape(&cid)),
            C_PHP_FIXTURE,
        );
        let reader_url = serde_json::from_str::<Value>(&cphp)
            .ok()
            .and_then(|root| root.get("url").and_then(Value::as_str).map(ToOwned::to_owned))
            .unwrap_or_else(|| format!("{BASE_URL}/viewer/sample?cid={cid}"));
        let body = fetch_document(&reader_url, READER_FIXTURE);
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: true,
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::SpeedBinb::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/content/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/{key}")))
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

#[derive(Clone, Debug)]
struct Product {
    id: String,
    name: String,
    start_date: Option<String>,
    end_date: Option<String>,
    download_url: String,
    sales_unit: Option<String>,
    series_id: String,
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_entries(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture"));
    let total = root.get("TotalResults").and_then(Value::as_u64).unwrap_or(0);
    let entries = root
        .get("Contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(content_to_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: total > entries.len() as u64,
        entries,
    }
}

fn content_to_item(content: &Value) -> Option<CatalogItem> {
    let key = text(content, "SeriesID")?;
    let tags = content
        .get("Tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Some(CatalogItem {
        key,
        title: text(content, "Name").unwrap_or_else(|| "Pash Up!".into()),
        cover: content.pointer("/Images/Series").and_then(Value::as_str).map(ToOwned::to_owned),
        authors: content
            .get("Writers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|writer| {
                let name = writer.get("name").and_then(Value::as_str)?;
                let role = writer.get("role_name").and_then(Value::as_str).unwrap_or("Writer");
                Some(format!("{role}: {name}"))
            })
            .collect(),
        description: text(content, "Explain").map(|value| html::strip_tags(&value)),
        status: if tags.iter().any(|tag| tag == "完結") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        tags,
        url: text(content, "SeriesID").map(|series| format!("{BASE_URL}/content/{series}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        extra: BTreeMap::from([(
            "pashCategory".into(),
            Value::String(text(content, "Category").unwrap_or_default()),
        )]),
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_json(
        &format!("{API_URL}/products.php?type=contents&limit=1&id={}&_={}", url::query_escape(key), timestamp_ms()),
        LIST_FIXTURE,
    );
    parse_entries(&body)
        .entries
        .into_iter()
        .next()
        .unwrap_or_else(|| CatalogItem {
            key: key.into(),
            title: "Pash Up!".into(),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            ..CatalogItem::default()
        })
}

fn chapter_list_body(series_id: &str) -> String {
    fetch_json(
        &format!(
            "{API_URL}/products.php?type=contents&id={}&unit=2&limit=9999&order=nodesc&_={}",
            url::query_escape(series_id),
            timestamp_ms()
        ),
        CHAPTERS_FIXTURE,
    )
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let now = manatan_extension::abi::system_time()
        .map(|time| time.unix_seconds)
        .unwrap_or(1_704_067_200);
    let mut products = collect_products(body)
        .into_iter()
        .filter(|product| {
            product
                .end_date
                .as_deref()
                .and_then(dates::parse_ymd)
                .map(|end| end > now)
                .unwrap_or(true)
        })
        .filter(|product| !hide_locked || !product.download_url.contains("/pageapi/download"))
        .collect::<Vec<_>>();
    products.sort_by(|a, b| {
        let unit_cmp = a.sales_unit.cmp(&b.sales_unit);
        if unit_cmp == std::cmp::Ordering::Equal {
            b.start_date.cmp(&a.start_date)
        } else {
            unit_cmp
        }
    });
    products
        .into_iter()
        .map(|product| {
            let locked = product.download_url.contains("/pageapi/download");
            MangaChapter {
                key: format!("{}#{}", product.series_id, product.id),
                title: Some(if locked { format!("Locked {}", product.name) } else { product.name }),
                date_uploaded: product.start_date.as_deref().and_then(dates::parse_ymd),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn collect_products(body: &str) -> Vec<Product> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture"));
    let mut products = Vec::<Product>::new();
    for content in root
        .get("Contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let series_id = text(content, "SeriesID").unwrap_or_default();
        if let Some(product) = content.get("Product").and_then(|value| product_from_value(value, &series_id)) {
            push_unique_product(&mut products, product);
        }
        if let Some(map) = content.get("ProductMinMax").and_then(Value::as_object) {
            for minmax in map.values() {
                for key in ["Min", "Max"] {
                    if let Some(product) = minmax.get(key).and_then(|value| product_from_value(value, &series_id)) {
                        push_unique_product(&mut products, product);
                    }
                }
            }
        }
    }
    products
}

fn product_from_value(value: &Value, series_id: &str) -> Option<Product> {
    Some(Product {
        id: text(value, "ID")?,
        name: text(value, "Name").unwrap_or_else(|| "Chapter".into()),
        start_date: text(value, "StartDate"),
        end_date: text(value, "EndDate"),
        download_url: text(value, "DownloadURL").unwrap_or_default(),
        sales_unit: text(value, "SalesUnit"),
        series_id: series_id.to_string(),
    })
}

fn push_unique_product(products: &mut Vec<Product>, product: Product) {
    if !products.iter().any(|existing| existing.id == product.id) {
        products.push(product);
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| value.as_bool().or_else(|| value.as_str().map(|text| text == "true")))
        .unwrap_or(false)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/content/"))
        .map(|value| value.trim_matches('/').to_string())
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1.split('#').next().unwrap_or_default();
    for part in query.split('&') {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

fn timestamp_ms() -> u64 {
    manatan_extension::abi::system_time()
        .map(|time| time.unix_seconds as u64 * 1000)
        .unwrap_or(1_704_067_200_000)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"TotalResults":1,"Contents":[{"SeriesID":"sample","Name":"Sample Pash Up!","Images":{"Series":"https://pash-up.jp/cover.jpg"},"Category":"2","Writers":[{"name":"Sample Author","role_name":"Author"}],"Explain":"<p>Sample description.</p>","Tags":["連載中"]}]}
"#;

const LATEST_FIXTURE: &str = LIST_FIXTURE;

const CHAPTERS_FIXTURE: &str = r#"
{"Contents":[{"SeriesID":"sample","Product":{"ID":"p1","Name":"Chapter 1","StartDate":"2024-01-01","EndDate":"","DownloadURL":"https://pash-up.jp/viewer?cid=samplecid","SalesUnit":"1"},"ProductMinMax":{}}]}
"#;

const C_PHP_FIXTURE: &str = r#"
{"url":"https://pash-up.jp/viewer/sample?cid=samplecid"}
"#;

const READER_FIXTURE: &str = r#"
<div id="content"><img data-ptimg="/sample.ptimg.json"></div>
"#;
