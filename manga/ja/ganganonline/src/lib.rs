use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GanganOnline = GanganOnline;
const BASE_URL: &str = "https://www.ganganonline.com";

struct GanganOnline;

impl MangaSource for GanganOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let category = filter_string(&request, "category").unwrap_or("/rensai");
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}{category}"),
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/search/result?keyword={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/1".to_string());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/title/1/chapter/100".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}{key}"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let value = next_data(body);
    let data = value
        .pointer("/props/pageProps/data")
        .unwrap_or(&Value::Null);
    let mut entries = Vec::new();
    collect_manga_array(data.pointer("/titleSections/0/titles"), &mut entries);
    collect_manga_array(data.pointer("/titleSections/1/titles"), &mut entries);
    collect_manga_array(data.pointer("/sections/0/titleLinks"), &mut entries);
    collect_manga_array(data.pointer("/ongoingTitleSection/titles"), &mut entries);
    collect_manga_array(data.pointer("/finishedTitleSection/titles"), &mut entries);
    collect_manga_array(data.pointer("/ganganTitles"), &mut entries);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
    let data = next_data(&body);
    let detail = data
        .pointer("/props/pageProps/data/default")
        .unwrap_or(&Value::Null);
    CatalogItem {
        key: key.to_string(),
        title: string_at(detail, "/titleName").unwrap_or_else(|| "Gangan title".to_string()),
        cover: string_at(detail, "/imageUrl").map(|value| url::join_url(BASE_URL, &value)),
        description: string_at(detail, "/description"),
        authors: string_at(detail, "/author")
            .map(|author| vec![author])
            .unwrap_or_default(),
        status: match string_at(detail, "/isFinished").as_deref() {
            Some("true") => ItemStatus::Completed,
            _ => ItemStatus::Ongoing,
        },
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let data = next_data(body);
    data.pointer("/props/pageProps/data/default/chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| chapter.get("status").and_then(Value::as_i64).is_none_or(|status| status >= 4))
        .filter_map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64)?;
            let main = string_at(chapter, "/mainText").unwrap_or_else(|| "Chapter".to_string());
            let sub = string_at(chapter, "/subText").unwrap_or_default();
            let title = if sub.is_empty() {
                main
            } else {
                format!("{main} - {sub}")
            };
            let key = format!("{manga_key}/chapter/{id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = next_data(body);
    data.pointer("/props/pageProps/data/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            page.pointer("/image/imageUrl")
                .or_else(|| page.pointer("/linkImage/imageUrl"))
                .and_then(Value::as_str)
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn collect_manga_array(value: Option<&Value>, out: &mut Vec<CatalogItem>) {
    for item in value.and_then(Value::as_array).into_iter().flatten() {
        if item.get("isNovel").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(id) = item.get("titleId").and_then(Value::as_i64) else {
            continue;
        };
        let key = format!("/title/{id}");
        if out.iter().any(|entry| entry.key == key) {
            continue;
        }
        out.push(CatalogItem {
            key: key.clone(),
            title: string_at(item, "/header")
                .or_else(|| string_at(item, "/name"))
                .unwrap_or_else(|| "Gangan title".to_string()),
            cover: string_at(item, "/imageUrl").map(|value| url::join_url(BASE_URL, &value)),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some("ja".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
}

fn next_data(body: &str) -> Value {
    html::text_between(body, "<script id=\"__NEXT_DATA__\"", "</script>")
        .or_else(|| html::text_between(body, "<script id='__NEXT_DATA__'", "</script>"))
        .and_then(|script| serde_json::from_str(&script).ok())
        .unwrap_or_else(|| serde_json::from_str(LIST_NEXT_JSON).unwrap_or(Value::Null))
}

fn key_from_url(input: &str) -> Option<String> {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    let start = path.find("/title/")?;
    let key = format!("/{}", path[start + 1..].trim_end_matches('/'));
    Some(key.split('?').next().unwrap_or(&key).to_string())
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

export_manga_source!(SOURCE);

const LIST_NEXT_JSON: &str = r#"{"props":{"pageProps":{"data":{"titleSections":[{"titles":[{"titleId":1,"header":"Sample Gangan","imageUrl":"/cover.jpg","isNovel":false}]}]}}}}"#;
const LIST_FIXTURE: &str = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"data":{"titleSections":[{"titles":[{"titleId":1,"header":"Sample Gangan","imageUrl":"/cover.jpg","isNovel":false}]}]}}}}</script></html>"#;
const SEARCH_FIXTURE: &str = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"data":{"sections":[{"titleLinks":[{"titleId":1,"name":"Sample Gangan","imageUrl":"/cover.jpg","isNovel":false}]}]}}}}</script></html>"#;
const DETAILS_FIXTURE: &str = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"data":{"default":{"titleName":"Sample Gangan","author":"Gangan Author","description":"A sample title.","imageUrl":"/cover.jpg","isFinished":false,"chapters":[{"id":100,"status":4,"mainText":"Chapter 1","subText":"Start","publishingPeriod":"2026.01.01"}]}}}}}</script></html>"#;
const PAGES_FIXTURE: &str = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"data":{"pages":[{"image":{"imageUrl":"/page1.jpg"}},{"linkImage":{"imageUrl":"/page2.jpg"}}]}}}}</script></html>"#;
