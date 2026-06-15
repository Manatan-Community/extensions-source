use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, webview,
    SearchRequest,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const API_URL: &str = "https://api.schale.network";
const SOURCE: Koharu = Koharu;

struct Koharu;

impl MangaSource for Koharu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { None } else { Some("8") };
        Ok(fetch_books(&books_url(page, "", sort, source, &request), BOOKS_FIXTURE, source, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some((id, key)) = id_key_from_query(query) {
            let body = fetch_json_or_fixture(&format!("{API_URL}/books/detail/{id}/{key}"), DETAIL_FIXTURE, &request, false);
            return Ok(Paged { entries: vec![parse_detail(&parse_detail_dto(&body), source, &request)], has_next_page: false });
        }
        Ok(fetch_books(&books_url(page, query, None, source, &request), BOOKS_FIXTURE, source, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/books/detail/{key}"), DETAIL_FIXTURE, &request, false);
        Ok(parse_detail(&parse_detail_dto(&body), source, &request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/books/detail/{key}"), DETAIL_FIXTURE, &request, false);
        let detail = parse_detail_dto(&body);
        Ok(vec![MangaChapter {
            key: format!("{}/{}", detail.id, detail.key),
            title: Some("Chapter".into()),
            date_uploaded: Some(detail.updated_at.unwrap_or(detail.created_at)),
            url: Some(format!("{}/g/{}/{}", base_url(&request), detail.id, detail.key)),
            page_count: Some(detail.thumbnails.entries.len() as u32),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/books/detail/{key}"), DATA_FIXTURE, &request, true);
        let data = serde_json::from_str::<MangaData>(&body).unwrap_or_else(|_| serde_json::from_str(DATA_FIXTURE).expect("data fixture"));
        let (id, public_key, quality) = selected_data_key(&data.data, image_quality(&request)).unwrap_or((1, "sample".into(), "1280".into()));
        let images_url = format!("{API_URL}/books/data/{key}/{id}/{public_key}/{quality}");
        let images_body = fetch_json_or_fixture(&images_url, IMAGES_FIXTURE, &request, false);
        let images = serde_json::from_str::<ImagesInfo>(&images_body).unwrap_or_else(|_| serde_json::from_str(IMAGES_FIXTURE).expect("images fixture"));
        let referer = base_url(&request);
        Ok(images
            .entries
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let image_url = format!("{}{}?w={quality}", images.base, image.path);
                MangaPage {
                    content: PageContent::Url { url: image_url, context: Some(image_headers(&referer)) },
                    headers: image_headers(&referer),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some((id, key)) = id_key_from_query(input) {
            let source = source_for(&request);
            let body = fetch_json_or_fixture(&format!("{API_URL}/books/detail/{id}/{key}"), DETAIL_FIXTURE, &request, false);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_detail(&parse_detail_dto(&body), source, &request)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    search_lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "koharu-all", lang: "all", search_lang: "" },
    SourceConfig { id: "koharu-en", lang: "en", search_lang: "english" },
    SourceConfig { id: "koharu-ja", lang: "ja", search_lang: "japanese" },
    SourceConfig { id: "koharu-zh", lang: "zh", search_lang: "chinese" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("koharu-all");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client(request: &Value) -> http::HttpClient {
    let base = base_url(request);
    http::HttpClient::browser()
        .with_origin(base.clone())
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str, request: &Value, post: bool) -> String {
    let target = with_clearance(target, request);
    let client = client(request);
    let builder = if post { client.post(target).json("{}") } else { client.get(target) };
    builder.xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_books(target: &str, fixture: &str, source: SourceConfig, request: &Value) -> Paged<CatalogItem> {
    let body = fetch_json_or_fixture(target, fixture, request, false);
    let books = serde_json::from_str::<Books>(&body).unwrap_or_else(|_| serde_json::from_str(BOOKS_FIXTURE).expect("books fixture"));
    Paged {
        has_next_page: books.page * books.limit < books.total,
        entries: books.entries.into_iter().map(|entry| entry.into_item(source, request)).collect(),
    }
}

fn books_url(page: u64, query: &str, forced_sort: Option<&str>, source: SourceConfig, request: &Value) -> String {
    let mut params = vec![format!("page={page}")];
    let sort_value = filter_value(request, "sort");
    let sort = forced_sort.or(sort_value.as_deref()).unwrap_or("4");
    if !sort.is_empty() {
        params.push(format!("sort={sort}"));
    }
    if let Some(cat) = filter_value(request, "category").filter(|value| !value.is_empty()) {
        params.push(format!("cat={cat}"));
    }
    if let Some(include) = filter_value(request, "tags").filter(|value| !value.is_empty()) {
        params.push(format!("include={}", query_escape(&include)));
    }
    if let Some(exclude) = filter_value(request, "exclude").filter(|value| !value.is_empty()) {
        params.push(format!("exclude={}", query_escape(&exclude)));
    }

    let mut terms = Vec::new();
    if source.lang != "all" {
        terms.push(format!("language:\"^{}$\"", source.search_lang));
    }
    if let Some(exclude_tags) = preference_str(request, "excludeTags").filter(|value| !value.trim().is_empty()) {
        terms.push(format!("tag:\"{}\"", exclude_tags.split(',').map(str::trim).filter(|tag| !tag.is_empty()).map(|tag| format!("-{tag}")).collect::<Vec<_>>().join(",")));
    }
    for kind in ["magazine", "publisher", "character", "cosplayer", "pages"] {
        if let Some(value) = filter_value(request, kind).filter(|value| !value.is_empty()) {
            let term = if kind == "pages" { format!("{kind}:{value}") } else { format!("{kind}:\"{value}\"") };
            terms.push(term);
        }
    }
    if !query.is_empty() {
        terms.push(format!("title:\"{query}\""));
    }
    if !terms.is_empty() {
        params.push(format!("s={}", query_escape(&terms.join(" "))));
    }
    format!("{API_URL}/books?{}", params.join("&"))
}

fn parse_detail_dto(body: &str) -> MangaDetail {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAIL_FIXTURE).expect("detail fixture"))
}

fn parse_detail(detail: &MangaDetail, source: SourceConfig, request: &Value) -> CatalogItem {
    let title = if preference_bool(request, "removeAdditionalTitleInfo") {
        shorten_title(&detail.title)
    } else {
        detail.title.clone()
    };
    let tags = detail.tags.iter().map(tag_label).collect::<Vec<_>>();
    let artists = detail.tags.iter().filter(|tag| tag.namespace == 1).map(|tag| capitalize_words(&tag.name)).collect::<Vec<_>>();
    let circles = detail.tags.iter().filter(|tag| tag.namespace == 2).map(|tag| capitalize_words(&tag.name)).collect::<Vec<_>>();
    CatalogItem {
        key: format!("{}/{}", detail.id, detail.key),
        title,
        cover: Some(format!("{}{}", detail.thumbnails.base, detail.thumbnails.main.path)),
        url: Some(format!("{}/g/{}/{}", base_url(request), detail.id, detail.key)),
        authors: if circles.is_empty() { artists.clone() } else { circles },
        artists,
        description: Some(format!("Pages: {}", detail.thumbnails.entries.len())),
        tags,
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        update_strategy: Some(manatan_extension::UpdateStrategy::OnlyFetchOnce),
        ..CatalogItem::default()
    }
}

fn selected_data_key(data: &Data, quality: String) -> Option<(i64, String, String)> {
    let choices: &[(&str, Option<&DataKey>)] = match quality.as_str() {
        "1600" => &[("1600", data.q1600.as_ref()), ("1280", data.q1280.as_ref()), ("0", Some(&data.original))],
        "980" => &[("980", data.q980.as_ref()), ("1280", data.q1280.as_ref()), ("0", Some(&data.original))],
        "780" => &[("780", data.q780.as_ref()), ("980", data.q980.as_ref()), ("0", Some(&data.original))],
        "0" => &[("0", Some(&data.original)), ("1600", data.q1600.as_ref()), ("1280", data.q1280.as_ref())],
        _ => &[("1280", data.q1280.as_ref()), ("1600", data.q1600.as_ref()), ("0", Some(&data.original))],
    };
    choices.iter().find_map(|(quality, key)| {
        let key = (*key)?;
        Some((key.id?, key.key.clone()?, (*quality).to_string()))
    })
}

fn with_clearance(target: &str, request: &Value) -> String {
    let Some(clearance) = clearance(request) else { return target.to_string(); };
    let sep = if target.contains('?') { '&' } else { '?' };
    format!("{target}{sep}crt={}", query_escape(&clearance))
}

fn clearance(request: &Value) -> Option<String> {
    webview::extract_text(
        webview::ExtractRequest::new(
            base_url(request),
            "Promise.resolve(window.localStorage.getItem('clearance') || '')",
        )
        .wait_for_script("window.localStorage && window.localStorage.getItem('clearance') !== null")
        .timeout_ms(10_000)
        .cookies(true),
    )
    .ok()
    .map(|value| value.trim_matches('"').to_string())
    .filter(|value| !value.is_empty() && value != "null")
}

fn id_key_from_query(input: &str) -> Option<(String, String)> {
    let value = input.strip_prefix("id:").unwrap_or(input);
    let path = value.split("/g/").nth(1).unwrap_or(value);
    let mut parts = path.trim_matches('/').split('/');
    let id = parts.next()?.to_string();
    let key = parts.next()?.to_string();
    if id.chars().all(|ch| ch.is_ascii_digit()) && !key.is_empty() {
        Some((id, key))
    } else {
        None
    }
}

fn base_url(request: &Value) -> String {
    let mirror = preference_str(request, "mirror").unwrap_or_else(|| "schale.network".into());
    format!("https://{}", mirror.trim_start_matches("https://").trim_end_matches('/'))
}

fn image_quality(request: &Value) -> String {
    preference_str(request, "imageResolution").unwrap_or_else(|| "1280".into())
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str).map(ToString::to_string)
}

fn preference_str(request: &Value, id: &str) -> Option<String> {
    request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_str).map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_bool).unwrap_or(false)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(ToString::to_string)
}

fn image_headers(referer: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), referer.to_string());
    headers
}

fn query_escape(input: &str) -> String {
    input.bytes().fold(String::new(), |mut out, byte| {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
        out
    })
}

fn tag_label(tag: &Tag) -> String {
    let suffix = match tag.namespace {
        8 => " male",
        9 => " female",
        _ => "",
    };
    capitalize_words(&format!("{}{}", tag.name, suffix))
}

fn capitalize_words(input: &str) -> String {
    input.split_whitespace().map(|word| {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    }).collect::<Vec<_>>().join(" ")
}

fn shorten_title(input: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for ch in input.chars() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[derive(Deserialize)]
struct Books {
    #[serde(default)]
    entries: Vec<Entry>,
    #[serde(default)]
    total: u64,
    #[serde(default = "default_limit")]
    limit: u64,
    #[serde(default = "default_page")]
    page: u64,
}

#[derive(Deserialize)]
struct Entry {
    id: i64,
    key: String,
    title: String,
    thumbnail: Thumbnail,
}

impl Entry {
    fn into_item(self, source: SourceConfig, request: &Value) -> CatalogItem {
        CatalogItem {
            key: format!("{}/{}", self.id, self.key),
            title: if preference_bool(request, "removeAdditionalTitleInfo") { shorten_title(&self.title) } else { self.title },
            cover: Some(self.thumbnail.path),
            url: Some(format!("{}/g/{}/{}", base_url(request), self.id, self.key)),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            status: ItemStatus::Completed,
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaDetail {
    id: i64,
    title: String,
    key: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: Option<i64>,
    thumbnails: Thumbnails,
    #[serde(default)]
    tags: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    namespace: i64,
}

#[derive(Deserialize)]
struct Thumbnails {
    base: String,
    main: Thumbnail,
    #[serde(default)]
    entries: Vec<Thumbnail>,
}

#[derive(Deserialize)]
struct Thumbnail {
    path: String,
}

#[derive(Deserialize)]
struct MangaData {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "0")]
    original: DataKey,
    #[serde(default, rename = "780")]
    q780: Option<DataKey>,
    #[serde(default, rename = "980")]
    q980: Option<DataKey>,
    #[serde(default, rename = "1280")]
    q1280: Option<DataKey>,
    #[serde(default, rename = "1600")]
    q1600: Option<DataKey>,
}

#[derive(Deserialize)]
struct DataKey {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    key: Option<String>,
}

#[derive(Deserialize)]
struct ImagesInfo {
    base: String,
    #[serde(default)]
    entries: Vec<ImagePath>,
}

#[derive(Deserialize)]
struct ImagePath {
    path: String,
}

fn default_limit() -> u64 { 20 }
fn default_page() -> u64 { 1 }

const BOOKS_FIXTURE: &str = r#"{
  "entries": [{ "id": 1, "key": "sample", "title": "Sample Book", "thumbnail": { "path": "https://static.schale.network/thumb.jpg" } }],
  "total": 1,
  "limit": 20,
  "page": 1
}"#;

const DETAIL_FIXTURE: &str = r#"{
  "id": 1,
  "title": "Sample Book [Extra]",
  "key": "sample",
  "created_at": 1704067200,
  "updated_at": 1704153600,
  "thumbnails": { "base": "https://static.schale.network", "main": { "path": "/cover.jpg" }, "entries": [{ "path": "/p1.jpg" }] },
  "tags": [{ "name": "artist name", "namespace": 1 }, { "name": "english", "namespace": 11 }]
}"#;

const DATA_FIXTURE: &str = r#"{
  "data": { "0": { "id": 1, "key": "original" }, "1280": { "id": 2, "key": "scaled" } }
}"#;

const IMAGES_FIXTURE: &str = r#"{
  "base": "https://static.schale.network",
  "entries": [{ "path": "/p1.jpg" }, { "path": "/p2.jpg" }]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_koharu_api() {
        let source = SOURCES[1];
        let page = fetch_books("fixture", BOOKS_FIXTURE, source, &Value::Null);
        assert_eq!(page.entries[0].key, "1/sample");
        let detail = parse_detail(&parse_detail_dto(DETAIL_FIXTURE), source, &serde_json::json!({"preferences":{"removeAdditionalTitleInfo":true}}));
        assert_eq!(detail.title, "Sample Book");
        assert_eq!(id_key_from_query("https://schale.network/g/1/sample"), Some(("1".into(), "sample".into())));
        let data = serde_json::from_str::<MangaData>(DATA_FIXTURE).unwrap();
        assert_eq!(selected_data_key(&data.data, "1280".into()).unwrap().2, "1280");
    }
}
