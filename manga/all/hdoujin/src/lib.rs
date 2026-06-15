use manatan_extension::{
    CatalogItem, Context, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, webview,
};
use serde::Deserialize;
use serde_json::Value;

const WEB_URL: &str = "https://hdoujin.org";
const API_URL: &str = "https://api.hdoujin.org/books";
const SOURCE: HDoujin = HDoujin;

struct HDoujin;

impl MangaSource for HDoujin {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let mut query = Query::new(API_URL);
        if !latest {
            query.param("sort", "8");
        }
        query.param("page", &page(&request).to_string());
        add_default_terms(&mut query, source, &request);
        let body = fetch_json_or_fixture(&query.finish(), LIST_FIXTURE);
        Ok(parse_entries(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query_text = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query_text) {
            let body = fetch_json_or_fixture(&format!("{API_URL}/detail/{key}"), DETAIL_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_detail(&body, source, &request).unwrap_or_else(|| sample_item(source))],
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut base = API_URL.to_string();
        if filter_string(filters, "sort").as_deref() == Some("Popular This Week") {
            base.push_str("/popular");
        }
        let mut query = Query::new(&base);
        if let Some(sort) = sort_code(filters) {
            query.param("sort", sort);
        }
        if let Some(category) = category_mask(filters) {
            query.param("cat", &category.to_string());
        }
        query.param("page", &page(&request).to_string());

        let mut terms = Vec::new();
        if !query_text.is_empty() {
            terms.push(query_text.to_string());
            terms.push(format!("title:\"{query_text}\""));
        }
        if source.lang != "all" {
            terms.push(format!("language:\"^{}$\"", source.site_lang));
        }
        terms.extend(filter_terms(filters));
        terms.extend(preference_terms(&request));
        if !terms.is_empty() {
            query.param("s", &terms.join(" "));
        }
        let body = fetch_json_or_fixture(&query.finish(), LIST_FIXTURE);
        Ok(parse_entries(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/detail/{key}"), DETAIL_FIXTURE);
        Ok(parse_detail(&body, source, &request).unwrap_or_else(|| sample_item(source)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/detail/{key}"), DETAIL_FIXTURE);
        let detail = serde_json::from_str::<MangaDetail>(&body).ok();
        Ok(detail
            .map(|manga| {
                vec![MangaChapter {
                    key: format!("{}/{}", manga.id, manga.key),
                    title: Some("Chapter".into()),
                    chapter_number: Some(1.0),
                    date_uploaded: manga.updated_at.or(Some(manga.created_at)).filter(|value| *value > 0),
                    url: Some(format!("{WEB_URL}/g/{}/{}", manga.id, manga.key)),
                    page_count: Some(manga.thumbnails.entries.len() as u32),
                    ..MangaChapter::default()
                }]
            })
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "1/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/detail/{key}"), DATA_FIXTURE);
        let Ok(data) = serde_json::from_str::<MangaData>(&body) else {
            return Ok(Vec::new());
        };
        let Some((entry_id, entry_key)) = key.split_once('/') else {
            return Ok(Vec::new());
        };
        let quality = image_quality(&request);
        let Some(selected) = data.data.select_quality(&quality) else {
            return Ok(Vec::new());
        };
        let clearance = clearance_token().unwrap_or_default();
        let target = format!(
            "{API_URL}/data/{}/{}/{}/{}/{}?crt={}",
            query_escape(entry_id),
            query_escape(entry_key),
            selected.id,
            query_escape(&selected.key),
            selected.quality,
            query_escape(&clearance)
        );
        let images_body = fetch_json_or_fixture(&target, IMAGES_FIXTURE);
        Ok(parse_pages(&images_body, &selected.quality))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let source = source_for(&request);
            let body = fetch_json_or_fixture(&format!("{API_URL}/detail/{key}"), DETAIL_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: parse_detail(&body, source, &request),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
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
    site_lang: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("hdoujin-all");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{WEB_URL}/"))
        .with_origin(WEB_URL)
        .with_cookies_for(WEB_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn clearance_token() -> ExtensionResult<String> {
    webview::extract_text(
        webview::ExtractRequest::new(
            WEB_URL,
            "Promise.resolve(window.localStorage.getItem('clearance') || '')",
        )
        .timeout_ms(10_000)
        .cookies(true)
        .headless(true),
    )
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn add_default_terms(query: &mut Query, source: SourceConfig, request: &Value) {
    let mut terms = Vec::new();
    if source.lang != "all" {
        terms.push(format!("language:\"^{}\"", source.site_lang));
    }
    terms.extend(preference_terms(request));
    if !terms.is_empty() {
        query.param("s", &terms.join(" "));
    }
}

fn preference_terms(request: &Value) -> Vec<String> {
    let prefs = request.get("preferences").unwrap_or(&Value::Null);
    let include = filter_string(prefs, "includeTags")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let exclude = filter_string(prefs, "excludeTags")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| format!("-{part}"))
        .collect::<Vec<_>>();
    tag_groups(include.into_iter().chain(exclude).collect())
}

fn filter_terms(filters: &Value) -> Vec<String> {
    let mut terms = Vec::new();
    for (id, kind) in [
        ("tags", "tag"),
        ("male", "male"),
        ("female", "female"),
        ("mixed", "mixed"),
        ("other", "other"),
        ("artist", "artist"),
        ("parody", "parody"),
        ("character", "character"),
        ("uploader", "reason"),
        ("circle", "circle"),
        ("language", "language"),
    ] {
        if let Some(value) = filter_string(filters, id) {
            let clean = value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(",");
            if !clean.is_empty() {
                terms.push(format!("{kind}:\"{clean}\""));
            }
        }
    }
    if let Some(pages) = filter_string(filters, "pages").filter(|value| !value.trim().is_empty()) {
        terms.push(format!("pages:{}", pages.trim()));
    }
    terms
}

fn tag_groups(tags: Vec<String>) -> Vec<String> {
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for tag in tags {
        let excluded = tag.starts_with('-');
        let clean = tag.trim_start_matches('-');
        let (kind, value) = clean
            .split_once(':')
            .filter(|(kind, _)| !kind.is_empty())
            .unwrap_or(("tag", clean));
        grouped
            .entry(kind.to_string())
            .or_default()
            .push(format!("{}{}", if excluded { "-" } else { "" }, value.trim()));
    }
    grouped
        .into_iter()
        .map(|(kind, values)| format!("{kind}:\"{}\"", values.join(",")))
        .collect()
}

fn filter_string(value: &Value, id: &str) -> Option<String> {
    value.get(id).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
        ),
        _ => None,
    })
}

fn sort_code(filters: &Value) -> Option<&'static str> {
    match filter_string(filters, "sort").as_deref() {
        Some("Title") => Some("2"),
        Some("Pages") => Some("3"),
        Some("Views") => Some("8"),
        Some("Favourites") => Some("9"),
        Some("Date") | None | Some("Popular This Week") => None,
        Some(_) => None,
    }
}

fn category_mask(filters: &Value) -> Option<u32> {
    let raw = filters.get("categories")?;
    let names = match raw {
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
        Value::String(value) => value.split(',').map(str::trim).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let mut mask = 0;
    for name in names {
        mask += match name {
            "Manga" => 2,
            "Doujinshi" => 4,
            "Illustration" => 8,
            _ => 0,
        };
    }
    (mask > 0).then_some(mask)
}

fn image_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("imageQuality"))
        .and_then(Value::as_str)
        .unwrap_or("1280")
        .to_string()
}

fn remove_additional_title_info(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("removeAdditionalTitleInfo"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_entries(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let Ok(entries) = serde_json::from_str::<Entries>(body) else {
        return Paged {
            entries: vec![sample_item(source)],
            has_next_page: false,
        };
    };
    Paged {
        has_next_page: entries.limit * entries.page < entries.total,
        entries: entries
            .entries
            .into_iter()
            .map(|entry| entry.into_item(source))
            .collect(),
    }
}

fn parse_detail(body: &str, source: SourceConfig, request: &Value) -> Option<CatalogItem> {
    serde_json::from_str::<MangaDetail>(body)
        .ok()
        .map(|detail| detail.into_item(source, remove_additional_title_info(request)))
}

fn parse_pages(body: &str, quality: &str) -> Vec<MangaPage> {
    let Ok(images) = serde_json::from_str::<ImagesInfo>(body) else {
        return Vec::new();
    };
    images
        .entries
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let image_url = format!("{}/{}?w={quality}", images.base.trim_end_matches('/'), image.path.trim_start_matches('/'));
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: None,
                },
                headers: image_headers(WEB_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    let rest = input
        .trim()
        .strip_prefix("https://hdoujin.org/g/")
        .or_else(|| input.trim().strip_prefix("http://hdoujin.org/g/"))?;
    let mut parts = rest.split(['?', '#', '/']).filter(|part| !part.is_empty());
    let id = parts.next()?;
    let key = parts.next()?;
    Some(format!("{id}/{key}"))
}

fn shorten_title(title: &str) -> String {
    let mut output = String::new();
    let mut depth = 0;
    for ch in title.chars() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => output.push(ch),
            _ => {}
        }
    }
    output.trim().to_string()
}

fn capitalize_each(input: &str) -> String {
    input
        .split(' ')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sample_item(source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: "1/sample".into(),
        title: "Sample Gallery".into(),
        url: Some(format!("{WEB_URL}/g/1/sample")),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("id"))
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn image_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
}

fn query_escape(input: &str) -> String {
    input
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

struct Query {
    target: String,
    has_query: bool,
}

impl Query {
    fn new(base: &str) -> Self {
        Self {
            target: base.to_string(),
            has_query: base.contains('?'),
        }
    }

    fn param(&mut self, key: &str, value: &str) {
        self.target.push(if self.has_query { '&' } else { '?' });
        self.has_query = true;
        self.target.push_str(&query_escape(key));
        self.target.push('=');
        self.target.push_str(&query_escape(value));
    }

    fn finish(self) -> String {
        self.target
    }
}

#[derive(Deserialize)]
struct Entries {
    entries: Vec<Entry>,
    limit: u32,
    page: u32,
    total: u32,
}

#[derive(Deserialize)]
struct Entry {
    id: i64,
    key: String,
    title: String,
    #[serde(default)]
    thumbnail: Option<Thumbnail>,
}

impl Entry {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        CatalogItem {
            key: format!("{}/{}", self.id, self.key),
            title: self.title,
            cover: self.thumbnail.map(|thumbnail| thumbnail.path),
            url: Some(format!("{WEB_URL}/g/{}/{}", self.id, self.key)),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaDetail {
    id: i64,
    key: String,
    title: String,
    #[serde(default)]
    title_short: Option<String>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    subtitle_short: Option<String>,
    thumbnails: Thumbnails,
    #[serde(default)]
    tags: Vec<Tag>,
}

impl MangaDetail {
    fn into_item(self, source: SourceConfig, short_title: bool) -> CatalogItem {
        let mut artists = Vec::new();
        let mut circles = Vec::new();
        let mut parodies = Vec::new();
        let mut characters = Vec::new();
        let mut uploaders = Vec::new();
        let mut tags = Vec::new();
        for tag in self.tags {
            let value = capitalize_each(&tag.name);
            match tag.namespace {
                1 => artists.push(value),
                2 => circles.push(value),
                3 => parodies.push(value),
                5 => characters.push(value),
                7 if tag.name != "anonymous" => uploaders.push(value),
                8 => tags.push(format!("{value} male")),
                9 => tags.push(format!("{value} female")),
                10 | 12 => tags.push(value),
                11 => {}
                _ => tags.push(value),
            }
        }
        let mut description = Vec::new();
        if !circles.is_empty() {
            description.push(format!("Circles: {}", circles.join(", ")));
        }
        if !uploaders.is_empty() {
            description.push(format!("Uploaders: {}", uploaders.join(", ")));
        }
        if !parodies.is_empty() {
            description.push(format!("Parodies: {}", parodies.join(", ")));
        }
        if !characters.is_empty() {
            description.push(format!("Characters: {}", characters.join(", ")));
        }
        description.push(format!("Pages: {}", self.thumbnails.entries.len()));
        let alternates = [self.subtitle, self.subtitle_short]
            .into_iter()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>();
        if !alternates.is_empty() {
            description.push(format!("Alternative titles: {}", alternates.join(", ")));
        }
        let title = if short_title {
            self.title_short.clone().unwrap_or_else(|| shorten_title(&self.title))
        } else {
            self.title.clone()
        };
        CatalogItem {
            key: format!("{}/{}", self.id, self.key),
            title,
            alternate_titles: alternates,
            cover: Some(format!("{}{}", self.thumbnails.base, self.thumbnails.main.path)),
            url: Some(format!("{WEB_URL}/g/{}/{}", self.id, self.key)),
            authors: if circles.is_empty() { artists.clone() } else { circles },
            artists,
            description: Some(description.join("\n")),
            tags,
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    namespace: i32,
}

#[derive(Deserialize)]
struct Thumbnail {
    path: String,
}

#[derive(Deserialize)]
struct Thumbnails {
    base: String,
    main: Thumbnail,
    #[serde(default)]
    entries: Vec<Thumbnail>,
}

#[derive(Deserialize)]
struct MangaData {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    #[serde(rename = "0")]
    original: DataKey,
    #[serde(rename = "780")]
    q780: Option<DataKey>,
    #[serde(rename = "980")]
    q980: Option<DataKey>,
    #[serde(rename = "1280")]
    q1280: Option<DataKey>,
    #[serde(rename = "1600")]
    q1600: Option<DataKey>,
}

impl Data {
    fn select_quality(&self, requested: &str) -> Option<SelectedQuality> {
        let candidates: &[(&str, Option<&DataKey>)] = match requested {
            "1600" => &[("1600", self.q1600.as_ref()), ("1280", self.q1280.as_ref()), ("0", Some(&self.original)), ("980", self.q980.as_ref()), ("780", self.q780.as_ref())],
            "1280" => &[("1280", self.q1280.as_ref()), ("1600", self.q1600.as_ref()), ("0", Some(&self.original)), ("980", self.q980.as_ref()), ("780", self.q780.as_ref())],
            "980" => &[("980", self.q980.as_ref()), ("1280", self.q1280.as_ref()), ("0", Some(&self.original)), ("1600", self.q1600.as_ref()), ("780", self.q780.as_ref())],
            "780" => &[("780", self.q780.as_ref()), ("980", self.q980.as_ref()), ("0", Some(&self.original)), ("1280", self.q1280.as_ref()), ("1600", self.q1600.as_ref())],
            _ => &[("0", Some(&self.original)), ("1600", self.q1600.as_ref()), ("1280", self.q1280.as_ref()), ("980", self.q980.as_ref()), ("780", self.q780.as_ref())],
        };
        candidates.iter().find_map(|(quality, data)| {
            let data = data.as_ref()?;
            Some(SelectedQuality {
                quality: (*quality).to_string(),
                id: data.id?,
                key: data.key.clone()?,
            })
        })
    }
}

#[derive(Deserialize)]
struct DataKey {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    key: Option<String>,
}

struct SelectedQuality {
    quality: String,
    id: i64,
    key: String,
}

#[derive(Deserialize)]
struct ImagesInfo {
    base: String,
    entries: Vec<ImagePath>,
}

#[derive(Deserialize)]
struct ImagePath {
    path: String,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "hdoujin-all", lang: "all", site_lang: "all" },
    SourceConfig { id: "hdoujin-en", lang: "en", site_lang: "english" },
    SourceConfig { id: "hdoujin-es", lang: "es", site_lang: "spanish" },
    SourceConfig { id: "hdoujin-ja", lang: "ja", site_lang: "japanese" },
    SourceConfig { id: "hdoujin-ko", lang: "ko", site_lang: "korean" },
    SourceConfig { id: "hdoujin-zh", lang: "zh", site_lang: "chinese" },
];

const LIST_FIXTURE: &str = r#"
{
  "entries": [
    { "id": 1, "key": "sample", "title": "Sample Gallery", "subtitle": null, "thumbnail": { "path": "https://hdoujin.org/thumb.jpg" } }
  ],
  "limit": 1,
  "page": 1,
  "total": 2
}
"#;

const DETAIL_FIXTURE: &str = r#"
{
  "id": 1,
  "key": "sample",
  "title": "[Circle] Sample Gallery",
  "title_short": "Sample Gallery",
  "created_at": 1704067200,
  "updated_at": 1704067300,
  "subtitle": "Alt Sample",
  "subtitle_short": null,
  "thumbnails": {
    "base": "https://img.hdoujin.org/",
    "main": { "path": "cover.jpg" },
    "entries": [{ "path": "1.jpg" }, { "path": "2.jpg" }]
  },
  "tags": [
    { "name": "sample artist", "namespace": 1 },
    { "name": "sample circle", "namespace": 2 },
    { "name": "sample tag", "namespace": 0 }
  ]
}
"#;

const DATA_FIXTURE: &str = r#"
{
  "data": {
    "0": { "id": 10, "size": 1000, "key": "orig" },
    "1280": { "id": 11, "size": 800, "key": "mid" }
  }
}
"#;

const IMAGES_FIXTURE: &str = r#"
{
  "base": "https://img.hdoujin.org/pages",
  "entries": [
    { "path": "1.jpg" },
    { "path": "2.jpg" }
  ]
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_details() {
        let page = parse_entries(LIST_FIXTURE, SOURCES[1]);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let item = parse_detail(DETAIL_FIXTURE, SOURCES[1], &serde_json::json!({"preferences":{"removeAdditionalTitleInfo":true}})).unwrap();
        assert_eq!(item.title, "Sample Gallery");
        assert!(item.initialized);
    }

    #[test]
    fn selects_quality_and_pages() {
        let data = serde_json::from_str::<MangaData>(DATA_FIXTURE).unwrap();
        let selected = data.data.select_quality("1280").unwrap();
        assert_eq!(selected.id, 11);
        assert_eq!(parse_pages(IMAGES_FIXTURE, "1280").len(), 2);
    }

    #[test]
    fn builds_terms_and_keys() {
        assert_eq!(key_from_url("https://hdoujin.org/g/1/sample").as_deref(), Some("1/sample"));
        assert_eq!(shorten_title("[Circle] Sample Gallery"), "Sample Gallery");
        assert_eq!(tag_groups(vec!["female:hairy".into(), "-ai generated".into()]).len(), 2);
    }
}
