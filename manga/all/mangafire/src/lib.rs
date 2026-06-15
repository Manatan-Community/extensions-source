use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

mod vrf;

use vrf::VrfGenerator;

const SOURCE: MangaFire = MangaFire;
const BASE_URL: &str = "https://mangafire.to";
const VOLUME_SUFFIX: &str = "#vol";
const VOLUME_PREFIX: &str = "[VOL] ";

struct MangaFire;

impl MangaSource for MangaFire {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recently_updated"
        } else {
            "most_viewed"
        };
        Ok(parse_search_page(&fetch_document(
            &filter_url(
                "",
                page(&request),
                source_lang_code(&request),
                sort,
                &request,
            ),
            SEARCH_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('"', " ")
            .trim()
            .to_string();
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let mut target = filter_url(
            &query,
            page(&request),
            source_lang_code(&request),
            &filter_string(&request, "sort").unwrap_or_else(|| "most_relevance".into()),
            &request,
        );
        if !query.is_empty() {
            target.push_str("&vrf=");
            target.push_str(&url::query_escape(&VrfGenerator::generate(&query)));
        }
        let mut page = parse_search_page(&fetch_document(&target, SEARCH_FIXTURE));
        if preference_bool(&request, "show_volume") {
            page.entries = with_volume_entries(page.entries);
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.1".into());
        let is_volume = key.ends_with(VOLUME_SUFFIX);
        let manga_id = manga_id(&key).unwrap_or_else(|| "1".into());
        let kind = if is_volume { "volume" } else { "chapter" };
        let lang_code = source_lang_code(&request);
        let target = format!("{BASE_URL}/ajax/manga/{manga_id}/{kind}/{lang_code}",);
        let read_vrf = VrfGenerator::generate(&format!("{manga_id}@{kind}@{lang_code}"));
        let read_target =
            format!("{BASE_URL}/ajax/read/{manga_id}/{kind}/{lang_code}?vrf={read_vrf}");
        let manga_list = fetch_json_result(&target, CHAPTERS_FIXTURE);
        let read_list = fetch_json_value(&read_target, READ_LIST_FIXTURE)
            .get("html")
            .and_then(Value::as_str)
            .unwrap_or(READ_LIST_HTML_FIXTURE)
            .to_string();
        Ok(parse_chapters(
            &manga_list,
            &read_list,
            is_volume,
            lang_code,
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter/1".into());
        let Some(read_key) = ajax_read_key(&key) else {
            return Ok(Vec::new());
        };
        let vrf = VrfGenerator::generate(&read_key.replace('/', "@"));
        let target = format!("{BASE_URL}/ajax/read/{read_key}?vrf={vrf}");
        let referer = request
            .get("chapter")
            .and_then(|chapter| chapter.get("url").or_else(|| chapter.get("realUrl")))
            .and_then(Value::as_str)
            .map(absolute_url)
            .unwrap_or_else(|| format!("{BASE_URL}/"));
        Ok(parse_pages(
            &fetch_json_result(&target, PAGES_FIXTURE),
            &referer,
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular =
            self.list(json!({"page": 1, "listingId": "popular", "sourceId": source_id(&request)}))?;
        let latest =
            self.list(json!({"page": 1, "listingId": "latest", "sourceId": source_id(&request)}))?;
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
        manga_image::MangaFireImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| absolute_url(&key.replace(VOLUME_SUFFIX, ""))))
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

fn fetch_json_result(target: &str, fixture: &str) -> String {
    let body = fetch_json_value(target, fixture);
    match body {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn fetch_json_value(target: &str, fixture: &str) -> Value {
    let body = client()
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("X-Requested-With", "XMLHttpRequest")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|root| root.get("result").cloned())
        .or_else(|| serde_json::from_str::<Value>(fixture).ok())
        .unwrap_or(Value::String(body))
}

fn filter_url(query: &str, page: u64, lang_code: &str, sort: &str, request: &Value) -> String {
    let mut params = Vec::<(String, String)>::new();
    if !query.is_empty() {
        params.push(("keyword".into(), query.into()));
    }
    for value in filter_array(request, "type") {
        params.push(("type[]".into(), value));
    }
    for value in filter_array(request, "genre") {
        params.push(("genre[]".into(), value));
    }
    if preference_bool(request, "genre_mode") || filter_bool(request, "genre_mode") {
        params.push(("genre_mode".into(), "and".into()));
    }
    for value in filter_array(request, "status") {
        params.push(("status[]".into(), value));
    }
    if let Some(minchap) = filter_string(request, "minchap").filter(|value| !value.is_empty()) {
        params.push(("minchap".into(), minchap));
    }
    params.push(("language[]".into(), lang_code.into()));
    params.push(("sort".into(), sort.into()));
    params.push(("page".into(), page.to_string()));
    format!("{BASE_URL}/filter?{}", encode_params(&params))
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("original card-lg")
        .skip(1)
        .flat_map(|chunk| chunk.split("class=\"unit").skip(1))
        .filter_map(search_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-item active")
            && body.contains("page-item")
            && body.contains("page-link"),
    }
}

fn search_item(chunk: &str) -> Option<CatalogItem> {
    let info = chunk.split("class=\"info").nth(1).unwrap_or(chunk);
    let href =
        html::attr_after(info, "<a", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(info, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaFire".into())),
        cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn with_volume_entries(entries: Vec<CatalogItem>) -> Vec<CatalogItem> {
    entries
        .into_iter()
        .flat_map(|item| {
            let mut volume = item.clone();
            volume.key = format!("{}{}", item.key, VOLUME_SUFFIX);
            volume.title = format!("{VOLUME_PREFIX}{}", item.title);
            vec![item, volume]
        })
        .collect()
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(
            &absolute_url(&key.replace(VOLUME_SUFFIX, "")),
            DETAILS_FIXTURE,
        ),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let main = body.split("main-inner").nth(1).unwrap_or(body);
    let mut description = html::text_between(body, "id=\"synopsis\"", "</div>")
        .or_else(|| html::text_between(body, "id='synopsis'", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    for label in ["Published", "Mangazine", "MAL"] {
        if let Some(value) = labeled_text(main, label) {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(label);
            description.push_str(": ");
            description.push_str(&value);
        }
    }
    let title = html::text_between(main, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "MangaFire".into()));
    CatalogItem {
        key: normalize_key(key),
        title: if key.ends_with(VOLUME_SUFFIX) {
            format!("{VOLUME_PREFIX}{title}")
        } else {
            title
        },
        cover: html::attr_after(main, "poster", "src")
            .or_else(|| html::attr_after(main, "<img", "src"))
            .map(|value| absolute_url(&value)),
        authors: labeled_text(main, "Author:")
            .into_iter()
            .flat_map(|value| split_csv(&value))
            .collect(),
        description: (!description.is_empty()).then_some(description),
        tags: labeled_text(main, "Genres:")
            .into_iter()
            .flat_map(|value| split_csv(&value))
            .collect(),
        status: match labeled_text(main, "Status:")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            value if value.contains("releasing") => ItemStatus::Ongoing,
            value if value.contains("completed") => ItemStatus::Completed,
            value if value.contains("hiatus") => ItemStatus::Hiatus,
            value if value.contains("discontinued") => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        url: Some(absolute_url(&key.replace(VOLUME_SUFFIX, ""))),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn labeled_text(body: &str, label: &str) -> Option<String> {
    let start = body.find(label)?;
    let chunk = &body[start + label.len()..];
    html::text_between(chunk, "<span", "</span>")
        .or_else(|| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_chapters(
    manga_body: &str,
    read_body: &str,
    is_volume: bool,
    lang_code: &str,
    manga_key: &str,
) -> Vec<MangaChapter> {
    let selector = if is_volume { "vol-list" } else { "<li" };
    let full_prefix = if is_volume { "Volume" } else { "Chapter" };
    let abbr_prefix = if is_volume { "Vol" } else { "Chap" };
    let kind = if is_volume { "volume" } else { "chapter" };
    let manga_pieces = if is_volume {
        manga_body.split("class=\"item").skip(1).collect::<Vec<_>>()
    } else {
        manga_body.split(selector).skip(1).collect::<Vec<_>>()
    };
    let read_pieces = if is_volume {
        read_body.split("<li").skip(1).collect::<Vec<_>>()
    } else {
        read_body.split("<li").skip(1).collect::<Vec<_>>()
    };
    let total = manga_pieces.len();
    manga_pieces
        .into_iter()
        .zip(read_pieces)
        .enumerate()
        .filter_map(|(index, (manga_chunk, read_chunk))| {
            let read_id = html::attr(read_chunk, "data-id")?;
            let number = html::attr(manga_chunk, "data-number")
                .or_else(|| html::attr(read_chunk, "data-number"))
                .unwrap_or_default();
            let href = html::attr_after(read_chunk, "<a", "href")
                .or_else(|| html::attr_after(manga_chunk, "<a", "href"))
                .unwrap_or_else(|| public_read_path(manga_key, kind, lang_code, &number));
            if let Some(read_number) = html::attr(read_chunk, "data-number")
                && !number.is_empty()
                && read_number != number
            {
                return None;
            }
            let raw_name = html::text_between(manga_chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("{full_prefix} {number}"));
            let prefix = format!("{abbr_prefix} {number}: ");
            let title = if raw_name.starts_with(&prefix) {
                let real = raw_name.trim_start_matches(&prefix);
                if real.contains(&number) {
                    raw_name
                } else {
                    format!("{full_prefix} {number}: {real}")
                }
            } else {
                raw_name
            };
            Some(MangaChapter {
                key: format!("{kind}/{read_id}"),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                date_uploaded: parse_date(manga_chunk),
                language: Some(lang_code.into()),
                url: Some(absolute_url(&href)),
                source_order: Some((total.saturating_sub(index)) as i32),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("pages fixture"));
    root.get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, image)| {
            let tuple = image.as_array()?;
            let image_url = tuple.first().and_then(Value::as_str)?.to_string();
            let offset = tuple.get(2).and_then(Value::as_u64).unwrap_or(0);
            let extra = if offset > 0 {
                BTreeMap::from([("mangaFireOffset".into(), json!(offset))])
            } else {
                BTreeMap::new()
            };
            Some(MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(image_context(referer)),
                },
                headers: image_context(referer),
                extra,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn image_context(referer: &str) -> Context {
    manga::image_headers(referer)
}

fn source_id(request: &Value) -> String {
    request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("mangafire-en")
        .to_string()
}

fn source_lang_code(request: &Value) -> &'static str {
    match source_id(request).as_str() {
        "mangafire-es" => "es",
        "mangafire-es-419" => "es-la",
        "mangafire-fr" => "fr",
        "mangafire-ja" => "ja",
        "mangafire-pt" => "pt",
        "mangafire-pt-br" => "pt-br",
        _ => "en",
    }
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn filter_bool(request: &Value, id: &str) -> bool {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn ajax_read_key(value: &str) -> Option<String> {
    let normalized = normalize_key(value);
    let trimmed = normalized.trim_start_matches('/');
    if trimmed.starts_with("chapter/") || trimmed.starts_with("volume/") {
        return Some(trimmed.to_string());
    }
    None
}

fn encode_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(value: &str) -> String {
    let suffix = value
        .ends_with(VOLUME_SUFFIX)
        .then_some(VOLUME_SUFFIX)
        .unwrap_or("");
    let without_suffix = value.trim_end_matches(VOLUME_SUFFIX);
    let path = without_suffix
        .strip_prefix(BASE_URL)
        .unwrap_or(without_suffix)
        .split('?')
        .next()
        .unwrap_or(without_suffix)
        .trim_end_matches('/');
    format!("/{}{}", path.trim_start_matches('/'), suffix)
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn public_read_path(manga_key: &str, kind: &str, lang_code: &str, number: &str) -> String {
    let slug = normalize_key(manga_key)
        .replace(VOLUME_SUFFIX, "")
        .trim_start_matches("/manga/")
        .to_string();
    format!("/read/{slug}/{lang_code}/{kind}-{number}")
}

fn manga_id(key: &str) -> Option<String> {
    key.replace(VOLUME_SUFFIX, "")
        .rsplit('.')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_date(chunk: &str) -> Option<i64> {
    let text = chunk
        .split("<span")
        .nth(2)
        .and_then(|value| html::text_between(value, ">", "</span>"))
        .map(|value| html::strip_tags(&value))?;
    parse_us_date(&text)
}

fn parse_us_date(value: &str) -> Option<i64> {
    let normalized = value.replace(',', "");
    let mut parts = normalized.split_whitespace();
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
<div class="original card-lg"><div class="unit"><div class="inner"><img src="/cover.jpg"><div class="info"><a href="/manga/sample.1">Sample MangaFire</a></div></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="main-inner"><h1>Sample MangaFire</h1><div class="poster"><img src="/cover.jpg"></div><div class="meta"><span>Author:</span><span>Sample Author</span><span>Status:</span><span>Releasing</span><span>Genres:</span><span>Action, Fantasy</span></div></div><div id="synopsis"><div class="modal-content">Sample description.</div></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<ul><li data-number="1"><a href="/read/sample.1/en/chapter-1"><span>Chap 1: Beginning</span><span>Jan 01, 2024</span></a></li></ul>
"#;

const READ_LIST_HTML_FIXTURE: &str = r#"
<ul><li data-id="1" data-number="1"><a href="/read/sample.1/en/chapter-1">Chapter 1</a></li></ul>
"#;

const READ_LIST_FIXTURE: &str = r#"
{"html":"<ul><li data-id=\"1\" data-number=\"1\"><a href=\"/read/sample.1/en/chapter-1\">Chapter 1</a></li></ul>"}
"#;

const PAGES_FIXTURE: &str = r#"
{"images":[["https://mangafire.to/image-1.jpg",1,0],["https://mangafire.to/image-2.jpg",2,3]]}
"#;
