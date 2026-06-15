use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: LineManga = LineManga;
const BASE_URL: &str = "https://manga.line.me";
const API_URL: &str = "https://manga.line.me/api";

struct LineManga;

impl MangaSource for LineManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/daily_list?week_day=2&page={page}&_=0")
        } else {
            format!("{API_URL}/periodic/gender_ranking?gender=0&page={page}&_=0")
        };
        Ok(parse_items(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_id(&key)],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!(
                "{API_URL}/search_product/list?word={}&page={page}&_=0",
                url::query_escape(query)
            )
        } else {
            let category = filter_string(&request, "category").unwrap_or("daily_list|2");
            let (kind, value) = category.split_once('|').unwrap_or(("daily_list", "2"));
            match kind {
                "daily_list" => format!("{API_URL}/daily_list?week_day={value}&page={page}&_=0"),
                "genre_list" => format!("{API_URL}/genre_list?genre_id={value}&page={page}&_=0"),
                _ => format!("{API_URL}/{kind}?gender={value}&page={page}&_=0"),
            }
        };
        Ok(parse_items(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "SAMPLE".into());
        Ok(details_by_id(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "SAMPLE".into());
        let hide_locked = preference_bool(&request, "hideLockedChapters", false);
        Ok(parse_chapters(&fetch_details_json(&key), &key, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "CHAPTER".into());
        Ok(parse_pages(&fetch_document(
            &format!("{BASE_URL}/book/viewer?id={key}"),
            PAGES_FIXTURE,
        )))
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
        manga_image::LineManga::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/product/periodic?id={key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/book/viewer?id={key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&key)),
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
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
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

fn fetch_details_json(id: &str) -> String {
    let target = if id.starts_with('E') {
        format!("{API_URL}/book/product_list?product_id={id}")
    } else {
        format!(
            "{API_URL}/book/product_list?need_read_info=1&rows=1000&is_periodic=1&product_id={id}"
        )
    };
    fetch_json(&target, DETAILS_FIXTURE)
}

fn parse_items(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or(Value::Null));
    let result = root.get("result").unwrap_or(&root);
    let entries = result
        .get("rows")
        .or_else(|| result.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("is_light_novel").and_then(Value::as_bool) != Some(true))
        .filter_map(catalog_from_item)
        .collect();
    Paged {
        entries,
        has_next_page: result
            .pointer("/pager/hasNext")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn catalog_from_item(item: &Value) -> Option<CatalogItem> {
    let id = text_value(item.get("id"))?;
    Some(CatalogItem {
        key: id.clone(),
        title: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Line Manga")
            .to_string(),
        cover: item
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        url: Some(format!("{BASE_URL}/product/periodic?id={id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_id(id: &str) -> CatalogItem {
    let root = serde_json::from_str::<Value>(&fetch_details_json(id))
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap_or(Value::Null));
    let product = root.pointer("/result/product").unwrap_or(&Value::Null);
    let authors = product
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    let title = product
        .get("series_name")
        .or_else(|| product.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Line Manga")
        .to_string();
    let mut description = Vec::new();
    for key in [
        "caption",
        "explanation",
        "periodic_description",
        "publisher_name",
    ] {
        if let Some(text) = product
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            description.push(text.to_string());
        }
    }
    CatalogItem {
        key: id.to_string(),
        title,
        cover: product
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        authors,
        description: (!description.is_empty()).then(|| description.join("\n\n")),
        tags: product
            .get("genre_name")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        url: Some(format!("{BASE_URL}/product/periodic?id={id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, product_id: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap_or(Value::Null));
    let rows = root
        .pointer("/result/rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut chapters = rows
        .iter()
        .filter_map(|row| {
            let id = text_value(row.get("id"))?;
            let locked = line_locked(row);
            if hide_locked && locked {
                return None;
            }
            let raw_name = row.get("name").and_then(Value::as_str).unwrap_or("Chapter");
            let series = row
                .get("series_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let title = raw_name.replace(series, "").trim().to_string();
            Some(MangaChapter {
                key: id.clone(),
                title: Some(format!("{}{}", if locked { "Locked " } else { "" }, title)),
                chapter_number: row
                    .get("volume")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                is_locked: locked,
                url: Some(format!("{BASE_URL}/book/viewer?id={id}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if !product_id.starts_with('E') {
        chapters.reverse();
    }
    chapters
}

fn line_locked(row: &Value) -> bool {
    if row.get("selling_buy_price").is_none() {
        return row.get("fin_of_purchase").and_then(Value::as_i64) != Some(1);
    }
    if !row.get("expired_on").unwrap_or(&Value::Null).is_null() {
        return false;
    }
    row.get("selling_buy_price")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        > 0
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = html::text_between(body, "var OPTION", "</script>")
        .map(|value| format!("var OPTION{value}"))
        .unwrap_or_else(|| body.to_string());
    if script.contains("isPortal") && script.contains("true") {
        return parse_portal_pages(&script);
    }
    script
        .split("imgs[")
        .skip(1)
        .filter_map(|chunk| {
            let url = quoted_after(chunk, "'url'").or_else(|| quoted_after(chunk, "\"url\""))?;
            (!url.contains("inline_ads_banner")).then_some(url)
        })
        .enumerate()
        .map(url_page)
        .collect()
}

fn parse_portal_pages(script: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    for chunk in script.split("portal_pages[").skip(1) {
        let Some((idx, rest)) = chunk.split_once(']') else {
            continue;
        };
        if !rest.trim_start().starts_with('=') {
            continue;
        }
        let index = idx.parse::<usize>().unwrap_or(pages.len());
        let Some(image) = quoted_after(rest, "'url'").or_else(|| quoted_after(rest, "\"url\""))
        else {
            continue;
        };
        let hc = number_after(rest, "'hc'").or_else(|| number_after(rest, "\"hc\""));
        let bwd = number_after(rest, "'bwd'").or_else(|| number_after(rest, "\"bwd\""));
        let m = metadata_values(script, idx);
        let mut extra = BTreeMap::new();
        if let (Some(hc), Some(bwd)) = (hc, bwd) {
            if !m.is_empty() {
                extra.insert("linePortal".into(), json!({"hc": hc, "bwd": bwd, "m": m}));
            }
        }
        pages.push(MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            extra,
            ..MangaPage::default()
        });
    }
    pages
}

fn url_page((index, image): (usize, String)) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn quoted_after(input: &str, marker: &str) -> Option<String> {
    let rest = input.split(marker).nth(1)?;
    let rest = rest.split(':').nth(1).unwrap_or(rest);
    let quote = rest.chars().find(|ch| *ch == '\'' || *ch == '"')?;
    let after = rest.split_once(quote)?.1;
    Some(after.split(quote).next()?.to_string())
}

fn number_after(input: &str, marker: &str) -> Option<u32> {
    let rest = input.split(marker).nth(1)?;
    let digits = rest
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn metadata_values(script: &str, idx: &str) -> Vec<String> {
    let marker = format!("portal_pages[{idx}].metadata.m[");
    let mut values = script
        .split(&marker)
        .skip(1)
        .filter_map(|chunk| quoted_after(chunk, "="))
        .collect::<Vec<_>>();
    if values.is_empty() {
        values = script
            .split(&marker)
            .skip(1)
            .filter_map(|chunk| {
                let quote = chunk.chars().find(|ch| *ch == '\'' || *ch == '"')?;
                Some(chunk.split(quote).nth(1)?.to_string())
            })
            .collect();
    }
    values
}

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)?
        .get(id)?
        .as_str()
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
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
        .unwrap_or(default)
}

fn key_from_url(input: &str) -> Option<String> {
    if let Some(query) = input.split("id=").nth(1) {
        Some(query.split('&').next().unwrap_or(query).to_string())
    } else {
        None
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"result":{"rows":[{"id":"SAMPLE","name":"Sample Line Manga","thumbnail":"https://img.example.test/cover.jpg","is_light_novel":false}],"pager":{"hasNext":false}}}"#;
const DETAILS_FIXTURE: &str = r#"{"result":{"product":{"name":"Sample Line Manga","series_name":"Sample Line Manga","thumbnail":"https://img.example.test/cover.jpg","authors":[{"name":"Sample Author"}],"genre_name":"Action","caption":"Sample caption.","explanation":"Sample description.","periodic_description":"","publisher_name":"Sample Publisher"},"rows":[{"id":"CHAPTER","name":"Sample Line Manga Chapter 1","series_name":"Sample Line Manga","volume":1,"selling_buy_price":0,"fin_of_purchase":1,"expired_on":null}]}}"#;
const PAGES_FIXTURE: &str = r#"<script>var OPTION = { isPortal: false }; imgs[0] = { 'url': 'https://img.example.test/page1.jpg' };</script>"#;
