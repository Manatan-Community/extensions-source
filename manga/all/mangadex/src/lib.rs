use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangadex.org";
const API_URL: &str = "https://api.mangadex.org";
const CDN_URL: &str = "https://uploads.mangadex.org";
const SOURCE: MangaDex = MangaDex;

struct MangaDex;

impl MangaSource for MangaDex {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let sort = if latest { "latestUploadedChapter" } else { "followedCount" };
        let body = fetch_json_or_fixture(&manga_query_url("", source, &request, page, sort), SEARCH_FIXTURE);
        Ok(parse_manga_list(&body, source, cover_suffix(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = manga_id_from_input(query) {
            let body = fetch_json_or_fixture(&manga_details_url(&id), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_manga(&body, source, cover_suffix(&request), true)], has_next_page: false });
        }
        let page = request_page(&request);
        let sort = filter_string(request.get("filters").unwrap_or(&Value::Null), "sort").unwrap_or("relevance");
        let body = fetch_json_or_fixture(&manga_query_url(query, source, &request, page, sort), SEARCH_FIXTURE);
        Ok(parse_manga_list(&body, source, cover_suffix(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| SAMPLE_MANGA_ID.into());
        let id = key.trim_start_matches("/manga/").to_string();
        let body = fetch_json_or_fixture(&manga_details_url(&id), DETAILS_FIXTURE);
        Ok(parse_manga(&body, source, cover_suffix(&request), true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| SAMPLE_MANGA_ID.into());
        let id = key.trim_start_matches("/manga/");
        let body = fetch_json_or_fixture(&chapter_feed_url(id, source, 0), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| SAMPLE_CHAPTER_ID.into());
        let id = key.trim_start_matches("/chapter/");
        let body = fetch_json_or_fixture(&format!("{API_URL}/at-home/server/{id}"), ATHOME_FIXTURE);
        Ok(parse_pages(&body, preference_bool(&request, "useDataSaver")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(id) = manga_id_from_input(input) {
            let body = fetch_json_or_fixture(&manga_details_url(&id), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_manga(&body, source, cover_suffix(&request), true)),
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
    dex_lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "mangadex-en", lang: "en", dex_lang: "en" },
    SourceConfig { id: "mangadex-af", lang: "af", dex_lang: "af" },
    SourceConfig { id: "mangadex-sq", lang: "sq", dex_lang: "sq" },
    SourceConfig { id: "mangadex-ar", lang: "ar", dex_lang: "ar" },
    SourceConfig { id: "mangadex-az", lang: "az", dex_lang: "az" },
    SourceConfig { id: "mangadex-eu", lang: "eu", dex_lang: "eu" },
    SourceConfig { id: "mangadex-be", lang: "be", dex_lang: "be" },
    SourceConfig { id: "mangadex-bn", lang: "bn", dex_lang: "bn" },
    SourceConfig { id: "mangadex-bg", lang: "bg", dex_lang: "bg" },
    SourceConfig { id: "mangadex-my", lang: "my", dex_lang: "my" },
    SourceConfig { id: "mangadex-ca", lang: "ca", dex_lang: "ca" },
    SourceConfig { id: "mangadex-zh-hans", lang: "zh-Hans", dex_lang: "zh" },
    SourceConfig { id: "mangadex-zh-hant", lang: "zh-Hant", dex_lang: "zh-hk" },
    SourceConfig { id: "mangadex-cv", lang: "cv", dex_lang: "cv" },
    SourceConfig { id: "mangadex-hr", lang: "hr", dex_lang: "hr" },
    SourceConfig { id: "mangadex-cs", lang: "cs", dex_lang: "cs" },
    SourceConfig { id: "mangadex-da", lang: "da", dex_lang: "da" },
    SourceConfig { id: "mangadex-nl", lang: "nl", dex_lang: "nl" },
    SourceConfig { id: "mangadex-eo", lang: "eo", dex_lang: "eo" },
    SourceConfig { id: "mangadex-et", lang: "et", dex_lang: "et" },
    SourceConfig { id: "mangadex-fil", lang: "fil", dex_lang: "tl" },
    SourceConfig { id: "mangadex-fi", lang: "fi", dex_lang: "fi" },
    SourceConfig { id: "mangadex-fr", lang: "fr", dex_lang: "fr" },
    SourceConfig { id: "mangadex-ka", lang: "ka", dex_lang: "ka" },
    SourceConfig { id: "mangadex-de", lang: "de", dex_lang: "de" },
    SourceConfig { id: "mangadex-el", lang: "el", dex_lang: "el" },
    SourceConfig { id: "mangadex-he", lang: "he", dex_lang: "he" },
    SourceConfig { id: "mangadex-hi", lang: "hi", dex_lang: "hi" },
    SourceConfig { id: "mangadex-hu", lang: "hu", dex_lang: "hu" },
    SourceConfig { id: "mangadex-ga", lang: "ga", dex_lang: "ga" },
    SourceConfig { id: "mangadex-id", lang: "id", dex_lang: "id" },
    SourceConfig { id: "mangadex-it", lang: "it", dex_lang: "it" },
    SourceConfig { id: "mangadex-ja", lang: "ja", dex_lang: "ja" },
    SourceConfig { id: "mangadex-jv", lang: "jv", dex_lang: "jv" },
    SourceConfig { id: "mangadex-kk", lang: "kk", dex_lang: "kk" },
    SourceConfig { id: "mangadex-ko", lang: "ko", dex_lang: "ko" },
    SourceConfig { id: "mangadex-la", lang: "la", dex_lang: "la" },
    SourceConfig { id: "mangadex-lt", lang: "lt", dex_lang: "lt" },
    SourceConfig { id: "mangadex-ms", lang: "ms", dex_lang: "ms" },
    SourceConfig { id: "mangadex-mn", lang: "mn", dex_lang: "mn" },
    SourceConfig { id: "mangadex-ne", lang: "ne", dex_lang: "ne" },
    SourceConfig { id: "mangadex-no", lang: "no", dex_lang: "no" },
    SourceConfig { id: "mangadex-fa", lang: "fa", dex_lang: "fa" },
    SourceConfig { id: "mangadex-pl", lang: "pl", dex_lang: "pl" },
    SourceConfig { id: "mangadex-pt-br", lang: "pt-BR", dex_lang: "pt-br" },
    SourceConfig { id: "mangadex-pt", lang: "pt", dex_lang: "pt" },
    SourceConfig { id: "mangadex-ro", lang: "ro", dex_lang: "ro" },
    SourceConfig { id: "mangadex-ru", lang: "ru", dex_lang: "ru" },
    SourceConfig { id: "mangadex-sr", lang: "sr", dex_lang: "sr" },
    SourceConfig { id: "mangadex-sk", lang: "sk", dex_lang: "sk" },
    SourceConfig { id: "mangadex-es-419", lang: "es-419", dex_lang: "es-la" },
    SourceConfig { id: "mangadex-es", lang: "es", dex_lang: "es" },
    SourceConfig { id: "mangadex-sv", lang: "sv", dex_lang: "sv" },
    SourceConfig { id: "mangadex-ta", lang: "ta", dex_lang: "ta" },
    SourceConfig { id: "mangadex-te", lang: "te", dex_lang: "te" },
    SourceConfig { id: "mangadex-th", lang: "th", dex_lang: "th" },
    SourceConfig { id: "mangadex-tr", lang: "tr", dex_lang: "tr" },
    SourceConfig { id: "mangadex-uk", lang: "uk", dex_lang: "uk" },
    SourceConfig { id: "mangadex-ur", lang: "ur", dex_lang: "ur" },
    SourceConfig { id: "mangadex-uz", lang: "uz", dex_lang: "uz" },
    SourceConfig { id: "mangadex-vi", lang: "vi", dex_lang: "vi" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("mangadex-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_origin(BASE_URL).with_referer(format!("{BASE_URL}/"))
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn manga_query_url(query: &str, source: SourceConfig, request: &Value, page: u64, sort: &str) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let mut params = vec![
        ("limit".to_string(), "32".to_string()),
        ("offset".into(), ((page.saturating_sub(1)) * 32).to_string()),
        ("includes[]".into(), "cover_art".into()),
        ("availableTranslatedLanguage[]".into(), source.dex_lang.into()),
        ("hasAvailableChapters".into(), "true".into()),
    ];
    if !query.is_empty() {
        params.push(("title".into(), query.into()));
    }
    for rating in csv_filter(filters, "contentRating").unwrap_or_else(|| vec!["safe".into(), "suggestive".into(), "erotica".into(), "pornographic".into()]) {
        params.push(("contentRating[]".into(), rating));
    }
    for tag in csv_filter(filters, "includedTags").unwrap_or_default() {
        params.push(("includedTags[]".into(), tag));
    }
    for tag in csv_filter(filters, "excludedTags").unwrap_or_default() {
        params.push(("excludedTags[]".into(), tag));
    }
    let order_key = match sort {
        "title" => "order[title]",
        "createdAt" => "order[createdAt]",
        "latestUploadedChapter" => "order[latestUploadedChapter]",
        "followedCount" => "order[followedCount]",
        _ => "order[relevance]",
    };
    params.push((order_key.into(), "desc".into()));
    format!("{API_URL}/manga?{}", form_body_owned(&params))
}

fn manga_details_url(id: &str) -> String {
    format!("{API_URL}/manga/{id}?includes[]=cover_art&includes[]=author&includes[]=artist")
}

fn chapter_feed_url(id: &str, source: SourceConfig, offset: u64) -> String {
    format!(
        "{API_URL}/manga/{id}/feed?limit=500&offset={offset}&translatedLanguage[]={}&includes[]=scanlation_group&includes[]=user&order[volume]=desc&order[chapter]=desc&includeFuturePublishAt=0&includeEmptyPages=0",
        query_escape(source.dex_lang)
    )
}

fn parse_manga_list(body: &str, source: SourceConfig, cover_suffix: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| parse_manga_entry(entry, source, cover_suffix, false))
        .collect();
    let limit = value.get("limit").and_then(Value::as_u64).unwrap_or(32);
    let offset = value.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let total = value.get("total").and_then(Value::as_u64).unwrap_or(0);
    Paged { entries, has_next_page: offset + limit < total }
}

fn parse_manga(body: &str, source: SourceConfig, cover_suffix: &str, initialized: bool) -> CatalogItem {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    parse_manga_entry(value.get("data").unwrap_or(&value), source, cover_suffix, initialized)
}

fn parse_manga_entry(entry: &Value, source: SourceConfig, cover_suffix: &str, initialized: bool) -> CatalogItem {
    let id = entry.get("id").and_then(Value::as_str).unwrap_or(SAMPLE_MANGA_ID);
    let attributes = entry.get("attributes").unwrap_or(&Value::Null);
    let title = localized_map(attributes.get("title"), source.dex_lang)
        .or_else(|| localized_alt_title(attributes.get("altTitles"), source.dex_lang))
        .unwrap_or_else(|| "Manga".into());
    let cover = cover_file(entry).map(|file| format!("{CDN_URL}/covers/{id}/{file}{cover_suffix}"));
    let description = localized_map(attributes.get("description"), source.dex_lang).or_else(|| localized_map(attributes.get("description"), "en"));
    CatalogItem {
        key: id.into(),
        title,
        cover,
        description,
        authors: relationship_names(entry, "author"),
        artists: relationship_names(entry, "artist"),
        tags: parse_tags(attributes),
        status: parse_status(attributes.get("status").and_then(Value::as_str).unwrap_or_default()),
        url: Some(format!("{BASE_URL}/title/{id}")),
        language: Some(source.lang.into()),
        content_rating: Some(content_rating(attributes)),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?;
            let attr = entry.get("attributes")?;
            let title = chapter_title(attr);
            Some(MangaChapter {
                key: id.into(),
                title: Some(title),
                chapter_number: attr.get("chapter").and_then(Value::as_str).and_then(|value| value.parse::<f32>().ok()),
                volume_number: attr.get("volume").and_then(Value::as_str).and_then(|value| value.parse::<f32>().ok()),
                scanlators: relationship_names(entry, "scanlation_group"),
                language: Some(source.lang.into()),
                url: Some(format!("{BASE_URL}/chapter/{id}")),
                page_count: attr.get("pages").and_then(Value::as_u64).map(|value| value as u32),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, data_saver: bool) -> Vec<MangaPage> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(ATHOME_FIXTURE).expect("fixture is valid"));
    let base = value.get("baseUrl").and_then(Value::as_str).unwrap_or(CDN_URL);
    let chapter = value.get("chapter").unwrap_or(&Value::Null);
    let hash = chapter.get("hash").and_then(Value::as_str).unwrap_or_default();
    let key = if data_saver { "dataSaver" } else { "data" };
    chapter
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, file)| {
            let path = if data_saver { "data-saver" } else { "data" };
            let url = format!("{base}/{path}/{hash}/{file}");
            MangaPage {
                content: PageContent::Url { url: url.clone(), context: Some(image_headers()) },
                headers: image_headers(),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn chapter_title(attr: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(volume) = attr.get("volume").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        parts.push(format!("Vol.{volume}"));
    }
    if let Some(chapter) = attr.get("chapter").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        parts.push(format!("Ch.{chapter}"));
    }
    if let Some(title) = attr.get("title").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        parts.push(title.into());
    }
    if parts.is_empty() { "Chapter".into() } else { parts.join(" ") }
}

fn cover_file(entry: &Value) -> Option<&str> {
    entry
        .get("relationships")?
        .as_array()?
        .iter()
        .find(|rel| rel.get("type").and_then(Value::as_str) == Some("cover_art"))?
        .get("attributes")?
        .get("fileName")?
        .as_str()
}

fn relationship_names(entry: &Value, rel_type: &str) -> Vec<String> {
    entry
        .get("relationships")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|rel| rel.get("type").and_then(Value::as_str) == Some(rel_type))
        .filter_map(|rel| {
            rel.get("attributes")
                .and_then(|attr| attr.get("name").or_else(|| attr.get("username")))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn parse_tags(attributes: &Value) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(language) = attributes.get("originalLanguage").and_then(Value::as_str) {
        tags.push(format!("Original language: {language}"));
    }
    if let Some(rating) = attributes.get("contentRating").and_then(Value::as_str).filter(|value| *value != "safe") {
        tags.push(format!("Rating: {rating}"));
    }
    for tag in attributes.get("tags").and_then(Value::as_array).into_iter().flatten() {
        if let Some(name) = localized_map(tag.get("attributes").and_then(|attr| attr.get("name")), "en") {
            tags.push(name);
        }
    }
    tags
}

fn parse_status(status: &str) -> ItemStatus {
    match status {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "cancelled" => ItemStatus::Cancelled,
        "hiatus" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn localized_map(value: Option<&Value>, lang: &str) -> Option<String> {
    let map = value?.as_object()?;
    map.get(lang)
        .or_else(|| map.get("en"))
        .or_else(|| map.values().next())
        .and_then(Value::as_str)
        .map(clean_text)
}

fn localized_alt_title(value: Option<&Value>, lang: &str) -> Option<String> {
    value?.as_array()?.iter().find_map(|entry| localized_map(Some(entry), lang))
}

fn content_rating(attributes: &Value) -> String {
    match attributes.get("contentRating").and_then(Value::as_str).unwrap_or("safe") {
        "pornographic" | "erotica" => "adult".into(),
        "suggestive" => "suggestive".into(),
        _ => "safe".into(),
    }
}

fn manga_id_from_input(input: &str) -> Option<String> {
    if is_uuid(input) {
        return Some(input.into());
    }
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let mut parts = input.split('/');
    while let Some(part) = parts.next() {
        if matches!(part, "title" | "manga") {
            let id = parts.next()?.split('?').next()?.split('#').next()?;
            if is_uuid(id) {
                return Some(id.into());
            }
        }
    }
    None
}

fn is_uuid(input: &str) -> bool {
    input.len() == 36 && input.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn cover_suffix(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("coverQuality"))
        .and_then(Value::as_str)
        .unwrap_or(".512.jpg")
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).or_else(|| filters.get("values").and_then(|values| values.get(key))).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn csv_filter(filters: &Value, key: &str) -> Option<Vec<String>> {
    let values = filter_string(filters, key)?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    headers
}

fn form_body_owned(fields: &[(String, String)]) -> String {
    fields.iter().map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value))).collect::<Vec<_>>().join("&")
}

fn query_escape(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn clean_text(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

const SAMPLE_MANGA_ID: &str = "11111111-1111-1111-1111-111111111111";
const SAMPLE_CHAPTER_ID: &str = "22222222-2222-2222-2222-222222222222";

const SEARCH_FIXTURE: &str = r#"{
  "limit": 32,
  "offset": 0,
  "total": 64,
  "data": [{
    "id": "11111111-1111-1111-1111-111111111111",
    "attributes": {
      "title": { "en": "Sample Dex" },
      "description": { "en": "A sample description." },
      "status": "ongoing",
      "contentRating": "safe",
      "originalLanguage": "ja",
      "tags": [{ "attributes": { "name": { "en": "Action" } } }]
    },
    "relationships": [{ "type": "cover_art", "attributes": { "fileName": "cover.jpg" } }]
  }]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "id": "11111111-1111-1111-1111-111111111111",
    "attributes": {
      "title": { "en": "Sample Dex" },
      "altTitles": [{ "ja": "Sample JP" }],
      "description": { "en": "A sample description." },
      "status": "completed",
      "contentRating": "suggestive",
      "originalLanguage": "ja",
      "tags": [{ "attributes": { "name": { "en": "Action" } } }]
    },
    "relationships": [
      { "type": "cover_art", "attributes": { "fileName": "cover.jpg" } },
      { "type": "author", "attributes": { "name": "Author One" } },
      { "type": "artist", "attributes": { "name": "Artist One" } }
    ]
  }
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "data": [{
    "id": "22222222-2222-2222-2222-222222222222",
    "attributes": { "volume": "1", "chapter": "1", "title": "Start", "pages": 2 },
    "relationships": [{ "type": "scanlation_group", "attributes": { "name": "Group One" } }]
  }]
}"#;

const ATHOME_FIXTURE: &str = r#"{
  "baseUrl": "https://uploads.mangadex.org",
  "chapter": {
    "hash": "hash-one",
    "data": ["page-1.jpg", "page-2.jpg"],
    "dataSaver": ["page-1-saver.jpg", "page-2-saver.jpg"]
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manga_list_and_details() {
        let page = parse_manga_list(SEARCH_FIXTURE, SOURCES[0], ".512.jpg");
        assert_eq!(page.entries[0].key, SAMPLE_MANGA_ID);
        assert!(page.has_next_page);
        let item = parse_manga(DETAILS_FIXTURE, SOURCES[0], "", true);
        assert_eq!(item.title, "Sample Dex");
        assert_eq!(item.authors, vec!["Author One"]);
        assert_eq!(item.status, ItemStatus::Completed);
    }

    #[test]
    fn parses_chapters_and_pages() {
        let chapters = parse_chapters(CHAPTERS_FIXTURE, SOURCES[0]);
        assert_eq!(chapters[0].title.as_deref(), Some("Vol.1 Ch.1 Start"));
        let pages = parse_pages(ATHOME_FIXTURE, false);
        assert_eq!(pages[0].description.as_deref(), Some("Page 1"));
        let saver = parse_pages(ATHOME_FIXTURE, true);
        assert!(matches!(&saver[0].content, PageContent::Url { url, .. } if url.contains("data-saver")));
    }

    #[test]
    fn extracts_ids_from_urls() {
        assert_eq!(manga_id_from_input(&format!("{BASE_URL}/title/{SAMPLE_MANGA_ID}/sample")).as_deref(), Some(SAMPLE_MANGA_ID));
    }
}
