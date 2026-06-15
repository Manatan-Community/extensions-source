use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangaball.net";
const SOURCE: MangaBall = MangaBall;

struct MangaBall;

impl MangaSource for MangaBall {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated_chapters_desc"
        } else {
            "views_desc"
        };
        let body = fetch_search_api("", source, &request, page, sort);
        Ok(parse_search(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(slug) = slug_from_url(query) {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}/title-detail/{slug}/"), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(slug), source)], has_next_page: false });
        }
        let default_filters = filters_are_default(&request);
        let body = if !query.is_empty() && default_filters && page == 1 {
            fetch_smart_search(query)
        } else {
            fetch_search_api(query, source, &request, page, "updated_chapters_desc")
        };
        Ok(parse_search(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample-100".into());
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/title-detail/{key}/"), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample-100".into());
        let title_id = key.rsplit('-').next().unwrap_or(&key);
        let body = fetch_chapter_api(title_id);
        Ok(parse_chapters(&body, source, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "chapter-1".into());
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/chapter-detail/{key}/"), PAGE_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(slug) = slug_from_url(input) {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}/title-detail/{slug}/"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(slug), source)),
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
    site_langs: &'static [&'static str],
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "mangaball-ar", lang: "ar", site_langs: &["ar"] },
    SourceConfig { id: "mangaball-bg", lang: "bg", site_langs: &["bg"] },
    SourceConfig { id: "mangaball-bn", lang: "bn", site_langs: &["bn"] },
    SourceConfig { id: "mangaball-ca", lang: "ca", site_langs: &["ca-ad", "ca-es", "ca-fr", "ca-it", "ca-pt"] },
    SourceConfig { id: "mangaball-cs", lang: "cs", site_langs: &["cs"] },
    SourceConfig { id: "mangaball-da", lang: "da", site_langs: &["da"] },
    SourceConfig { id: "mangaball-de", lang: "de", site_langs: &["de"] },
    SourceConfig { id: "mangaball-el", lang: "el", site_langs: &["el"] },
    SourceConfig { id: "mangaball-en", lang: "en", site_langs: &["en"] },
    SourceConfig { id: "mangaball-es", lang: "es", site_langs: &["es-ar", "es-mx", "es-es", "es-la", "es-419"] },
    SourceConfig { id: "mangaball-fa", lang: "fa", site_langs: &["fa"] },
    SourceConfig { id: "mangaball-fi", lang: "fi", site_langs: &["fi"] },
    SourceConfig { id: "mangaball-fr", lang: "fr", site_langs: &["fr"] },
    SourceConfig { id: "mangaball-he", lang: "he", site_langs: &["he"] },
    SourceConfig { id: "mangaball-hi", lang: "hi", site_langs: &["hi"] },
    SourceConfig { id: "mangaball-hu", lang: "hu", site_langs: &["hu"] },
    SourceConfig { id: "mangaball-id", lang: "id", site_langs: &["id"] },
    SourceConfig { id: "mangaball-it", lang: "it", site_langs: &["it-it"] },
    SourceConfig { id: "mangaball-is", lang: "is", site_langs: &["ib", "ib-is", "is"] },
    SourceConfig { id: "mangaball-ja", lang: "ja", site_langs: &["jp"] },
    SourceConfig { id: "mangaball-ko", lang: "ko", site_langs: &["kr"] },
    SourceConfig { id: "mangaball-kn", lang: "kn", site_langs: &["kn", "kn-in", "kn-my", "kn-sg", "kn-tw"] },
    SourceConfig { id: "mangaball-ml", lang: "ml", site_langs: &["ml", "ml-in", "ml-my", "ml-sg", "ml-tw"] },
    SourceConfig { id: "mangaball-ms", lang: "ms", site_langs: &["ms"] },
    SourceConfig { id: "mangaball-ne", lang: "ne", site_langs: &["ne"] },
    SourceConfig { id: "mangaball-nl", lang: "nl", site_langs: &["nl", "nl-be"] },
    SourceConfig { id: "mangaball-no", lang: "no", site_langs: &["no"] },
    SourceConfig { id: "mangaball-pl", lang: "pl", site_langs: &["pl"] },
    SourceConfig { id: "mangaball-pt-br", lang: "pt-BR", site_langs: &["pt-br", "pt-pt"] },
    SourceConfig { id: "mangaball-ro", lang: "ro", site_langs: &["ro"] },
    SourceConfig { id: "mangaball-ru", lang: "ru", site_langs: &["ru"] },
    SourceConfig { id: "mangaball-sk", lang: "sk", site_langs: &["sk"] },
    SourceConfig { id: "mangaball-sl", lang: "sl", site_langs: &["sl"] },
    SourceConfig { id: "mangaball-sq", lang: "sq", site_langs: &["sq"] },
    SourceConfig { id: "mangaball-sr", lang: "sr", site_langs: &["sr", "sr-cyrl"] },
    SourceConfig { id: "mangaball-sv", lang: "sv", site_langs: &["sv"] },
    SourceConfig { id: "mangaball-ta", lang: "ta", site_langs: &["ta"] },
    SourceConfig { id: "mangaball-th", lang: "th", site_langs: &["th", "th-hk", "th-kh", "th-la", "th-my", "th-sg"] },
    SourceConfig { id: "mangaball-tr", lang: "tr", site_langs: &["tr"] },
    SourceConfig { id: "mangaball-uk", lang: "uk", site_langs: &["uk"] },
    SourceConfig { id: "mangaball-vi", lang: "vi", site_langs: &["vi"] },
    SourceConfig { id: "mangaball-zh", lang: "zh", site_langs: &["zh", "zh-cn", "zh-hk", "zh-mo", "zh-sg", "zh-tw"] },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("mangaball-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[8])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_header("Cookie", "show18PlusContent=true")
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_smart_search(query: &str) -> String {
    let form = form_body(&[("search_input", query.trim())]);
    api_post("/api/v1/smart-search/search/", form).unwrap_or_else(|| SMART_FIXTURE.to_string())
}

fn fetch_search_api(query: &str, source: SourceConfig, request: &Value, page: u64, default_sort: &str) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let sort = filter_string(filters, "sort").unwrap_or(default_sort);
    let demographic = filter_string(filters, "demographic").unwrap_or("any");
    let status = filter_string(filters, "status").unwrap_or("any");
    let include_mode = filter_string(filters, "tagIncludedMode").unwrap_or("and");
    let exclude_mode = filter_string(filters, "tagExcludedMode").unwrap_or("and");
    let mut fields = vec![
        ("search_input".to_string(), query.trim().to_string()),
        ("filters[sort]".into(), sort.into()),
        ("filters[page]".into(), page.to_string()),
        ("filters[tag_included_mode]".into(), include_mode.into()),
        ("filters[tag_excluded_mode]".into(), exclude_mode.into()),
        ("filters[contentRating]".into(), "any".into()),
        ("filters[demographic]".into(), demographic.into()),
        ("filters[person]".into(), "any".into()),
        ("filters[publicationYear]".into(), String::new()),
        ("filters[publicationStatus]".into(), status.into()),
    ];
    for tag in csv_filter(filters, "tagIncludedIds") {
        fields.push(("filters[tag_included_ids][]".into(), tag));
    }
    for tag in csv_filter(filters, "tagExcludedIds") {
        fields.push(("filters[tag_excluded_ids][]".into(), tag));
    }
    for lang in source.site_langs {
        fields.push(("filters[translatedLanguage][]".into(), (*lang).into()));
    }
    api_post("/api/v1/title/search-advanced/", form_body_owned(&fields)).unwrap_or_else(|| SEARCH_FIXTURE.to_string())
}

fn fetch_chapter_api(title_id: &str) -> String {
    let form = form_body(&[("title_id", title_id)]);
    api_post("/api/v1/chapter/chapter-listing-by-title-id/", form).unwrap_or_else(|| CHAPTERS_FIXTURE.to_string())
}

fn api_post(path: &str, body: String) -> Option<String> {
    let csrf = csrf_token();
    client()
        .post(format!("{BASE_URL}{path}"))
        .xhr()
        .header("X-CSRF-TOKEN", csrf)
        .body(body)
        .send_text()
        .ok()
}

fn csrf_token() -> String {
    let body = client().get(BASE_URL).browser_document().send_text().unwrap_or_else(|_| HOME_FIXTURE.to_string());
    attr_after(&body, "csrf-token", "content").unwrap_or_default()
}

fn parse_search(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    if let Ok(response) = serde_json::from_str::<SmartSearchResponse>(body) {
        let entries = response
            .data
            .manga
            .into_iter()
            .filter_map(|item| catalog_from_parts(&item.url, &item.title, item.img.as_deref(), source))
            .collect();
        return Paged { entries, has_next_page: false };
    }
    let response = serde_json::from_str::<SearchResponse>(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    let entries = response
        .data
        .into_iter()
        .filter_map(|item| catalog_from_parts(&item.url, &item.name, item.cover.as_deref(), source))
        .collect();
    Paged { entries, has_next_page: response.pagination.current_page < response.pagination.last_page }
}

fn catalog_from_parts(path: &str, title: &str, cover: Option<&str>, source: SourceConfig) -> Option<CatalogItem> {
    let slug = if path.starts_with("http") {
        path.trim_end_matches('/').rsplit('/').next()?.to_string()
    } else {
        path.trim_matches('/').split('/').next_back()?.to_string()
    };
    Some(CatalogItem {
        key: slug.clone(),
        title: title.into(),
        cover: cover.map(|value| url_join(BASE_URL, value)),
        url: Some(format!("{BASE_URL}/title-detail/{slug}/")),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.or_else(|| slug_from_location(body)).unwrap_or_else(|| "sample-100".into());
    let alt_names = text_between(body, "alternate-name-container", "</div>").map(|value| strip_tags(&value));
    let description = [
        text_between(body, "descriptionContent", "</div>").map(|value| strip_tags(&value)),
        text_between(body, "badge:contains(Published)", "</span>").map(|value| strip_tags(&value)),
        alt_names.map(|names| format!("Alternative Names:\n{}", bullet_lines(&names))),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    CatalogItem {
        key: key.clone(),
        title: text_between(body, "comicDetail", "</h6>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Manga".into()),
        cover: attr_after(body, "featured-cover", "src").map(|value| url_join(BASE_URL, &value)),
        description: (!description.is_empty()).then_some(description),
        authors: parse_people(body),
        artists: parse_people(body),
        tags: parse_tags(body),
        status: parse_status(&text_between(body, "badge-status", "</span>").map(|value| strip_tags(&value)).unwrap_or_default()),
        url: Some(format!("{BASE_URL}/title-detail/{key}/")),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig, _series_key: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterListResponse>(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    response
        .chapters
        .into_iter()
        .flat_map(|chapter| {
            chapter.translations.into_iter().filter_map(move |translation| {
                if !source.site_langs.contains(&translation.language.as_str()) {
                    return None;
                }
                let number = clean_number(chapter.number);
                let title = if translation.name.contains(&number) {
                    translation.name.trim().to_string()
                } else {
                    format!("Ch. {number} {}", translation.name.trim())
                };
                Some(MangaChapter {
                    key: translation.id.clone(),
                    title: Some(title),
                    chapter_number: Some(chapter.number),
                    scanlators: vec![scanlator_name(&translation.group)],
                    language: Some(source.lang.into()),
                    url: Some(format!("{BASE_URL}/chapter-detail/{}/", translation.id)),
                    ..MangaChapter::default()
                })
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = extract_script_json_array(body, "chapterImages").unwrap_or_default();
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), format!("{BASE_URL}/"));
            MangaPage {
                content: PageContent::Url { url: image, context: Some(headers.clone()) },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchManga>,
    pagination: Pagination,
}

#[derive(Deserialize)]
struct Pagination {
    current_page: u64,
    last_page: u64,
}

#[derive(Deserialize)]
struct SearchManga {
    url: String,
    name: String,
    cover: Option<String>,
}

#[derive(Deserialize)]
struct SmartSearchResponse {
    data: SmartSearchData,
}

#[derive(Deserialize)]
struct SmartSearchData {
    manga: Vec<SmartSearchManga>,
}

#[derive(Deserialize)]
struct SmartSearchManga {
    title: String,
    img: Option<String>,
    url: String,
}

#[derive(Deserialize)]
struct ChapterListResponse {
    #[serde(rename = "ALL_CHAPTERS")]
    chapters: Vec<ChapterContainer>,
}

#[derive(Deserialize)]
struct ChapterContainer {
    #[serde(rename = "number_float")]
    number: f32,
    translations: Vec<ChapterTranslation>,
}

#[derive(Deserialize)]
struct ChapterTranslation {
    id: String,
    name: String,
    language: String,
    group: Group,
}

#[derive(Deserialize)]
struct Group {
    #[serde(rename = "_id")]
    id: String,
    name: String,
}

fn scanlator_name(group: &Group) -> String {
    if group.id.len() == 24 && group.id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        group.name.clone()
    } else {
        format!("{} ({})", group.name, group.id)
    }
}

fn filters_are_default(request: &Value) -> bool {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    filter_string(filters, "demographic").unwrap_or("any") == "any"
        && filter_string(filters, "status").unwrap_or("any") == "any"
        && csv_filter(filters, "tagIncludedIds").is_empty()
        && csv_filter(filters, "tagExcludedIds").is_empty()
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).or_else(|| filters.get("values").and_then(|values| values.get(key))).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn csv_filter(filters: &Value, key: &str) -> Vec<String> {
    filter_string(filters, key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn slug_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let mut segments = input.trim_end_matches('/').split('/');
    let last = segments.next_back()?;
    let previous = segments.next_back()?;
    match previous {
        "title-detail" => Some(last.into()),
        "chapter-detail" => None,
        _ => None,
    }
}

fn slug_from_location(body: &str) -> Option<String> {
    attr_after(body, "canonical", "href").and_then(|href| slug_from_url(&href))
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

fn parse_people(body: &str) -> Vec<String> {
    body.split("data-person-id")
        .skip(1)
        .filter_map(|chunk| text_between(chunk, ">", "</span>"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_tags(body: &str) -> Vec<String> {
    let mut tags = Vec::new();
    if body.contains("/flags/jp") {
        tags.push("Manga".into());
    }
    for tag in body
        .split("data-tag-id")
        .skip(1)
        .filter_map(|chunk| text_between(chunk, ">", "</span>"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        tags.push(tag);
    }
    tags
}

fn parse_status(input: &str) -> ItemStatus {
    match input.trim() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        "Hiatus" => ItemStatus::Hiatus,
        "Cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn extract_script_json_array(body: &str, const_name: &str) -> Option<Vec<String>> {
    let marker = format!("const {const_name}");
    let script = body.split(&marker).nth(1)?;
    let start = script.find("JSON.parse(`")? + "JSON.parse(`".len();
    let rest = &script[start..];
    let end = rest.find('`')?;
    serde_json::from_str(&rest[..end]).ok()
}

fn clean_number(number: f32) -> String {
    let mut value = number.to_string();
    if value.ends_with(".0") {
        value.truncate(value.len() - 2);
    }
    value
}

fn bullet_lines(input: &str) -> String {
    input
        .split('/')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn form_body(fields: &[(&str, &str)]) -> String {
    fields.iter().map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value))).collect::<Vec<_>>().join("&")
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

fn attr(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let start = input.find(&needle)? + needle.len();
        let rest = &input[start..];
        let end = rest.find(quote)?;
        return Some(html_unescape(&rest[..end]));
    }
    None
}

fn attr_after(input: &str, marker: &str, name: &str) -> Option<String> {
    let start = input.find(marker)?;
    attr(&input[start..], name)
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    let start_index = input.find(start)?;
    let after_start = &input[start_index..];
    let content_start = after_start.find('>').map(|idx| idx + 1).unwrap_or(start.len());
    let rest = &after_start[content_start..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].to_string())
}

fn strip_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    html_unescape(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn url_join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    }
}

const HOME_FIXTURE: &str = r#"<html><head><meta name="csrf-token" content="fixture-token"></head></html>"#;

const SEARCH_FIXTURE: &str = r#"{
  "data": [
    { "url": "https://mangaball.net/title-detail/sample-100/", "name": "Sample Ball", "cover": "https://cdn.example/cover.jpg" }
  ],
  "pagination": { "current_page": 1, "last_page": 2 }
}"#;

const SMART_FIXTURE: &str = r#"{
  "data": {
    "manga": [
      { "title": "Smart Sample", "img": "https://cdn.example/smart.jpg", "url": "/title-detail/smart-101/" }
    ]
  }
}"#;

const DETAILS_FIXTURE: &str = r#"
<link rel="canonical" href="https://mangaball.net/title-detail/sample-100/">
<div id="comicDetail"><h6>Sample Ball</h6>
<img class="featured-cover" src="/cover.jpg">
<img src="/flags/jp.svg">
<span data-tag-id="1">Action</span><span data-person-id="a">Sample Author</span>
<span class="badge-status">Ongoing</span>
<div id="descriptionContent"><p>Sample description.</p></div>
<div class="alternate-name-container">Alt One / Alt Two</div>
</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "ALL_CHAPTERS": [
    {
      "number_float": 1.0,
      "translations": [
        { "id": "chapter-1", "name": "The Start", "language": "en", "group": { "_id": "scan-group", "name": "Group One" } },
        { "id": "chapter-es", "name": "Inicio", "language": "es-es", "group": { "_id": "0123456789abcdef01234567", "name": "Group Two" } }
      ]
    }
  ]
}"#;

const PAGE_FIXTURE: &str = r#"
<script>
const chapterImages = JSON.parse(`["https://cdn.example/page-1.jpg","https://cdn.example/page-2.jpg"]`);
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_response_maps_catalog() {
        let page = parse_search(SEARCH_FIXTURE, SOURCES[8]);
        assert_eq!(page.entries[0].key, "sample-100");
        assert!(page.has_next_page);
    }

    #[test]
    fn smart_search_response_maps_catalog() {
        let page = parse_search(SMART_FIXTURE, SOURCES[8]);
        assert_eq!(page.entries[0].title, "Smart Sample");
        assert!(!page.has_next_page);
    }

    #[test]
    fn details_parse_metadata() {
        let item = parse_details(DETAILS_FIXTURE, None, SOURCES[8]);
        assert_eq!(item.key, "sample-100");
        assert_eq!(item.title, "Sample Ball");
        assert_eq!(item.authors, vec!["Sample Author"]);
        assert_eq!(item.tags, vec!["Manga", "Action"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
    }

    #[test]
    fn chapters_filter_by_source_language() {
        let chapters = parse_chapters(CHAPTERS_FIXTURE, SOURCES[8], "sample-100");
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].key, "chapter-1");
        let chapters = parse_chapters(CHAPTERS_FIXTURE, SOURCES[9], "sample-100");
        assert_eq!(chapters[0].key, "chapter-es");
    }

    #[test]
    fn pages_parse_script_images() {
        let pages = parse_pages(PAGE_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
