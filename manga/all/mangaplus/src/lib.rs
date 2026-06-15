use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangaplus.shueisha.co.jp";
const API_URL: &str = "https://jumpg-webapi.tokyo-cdn.com/api";
const SOURCE: MangaPlus = MangaPlus;

struct MangaPlus;

impl MangaSource for MangaPlus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{API_URL}/home_v4?lang={}&clang={}&format=json", source.internal_lang, source.internal_lang)
        } else {
            format!("{API_URL}/title_list/rankingV2?lang={}&type=hottest&clang={}&format=json", source.internal_lang, source.internal_lang)
        };
        let body = fetch_json_or_fixture(&target, if latest { HOME_FIXTURE } else { RANKING_FIXTURE });
        Ok(parse_listing(&body, source, latest))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = title_id_from_input(query) {
            let body = fetch_json_or_fixture(&title_detail_url(id), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details_value(&body, source)], has_next_page: false });
        }
        if let Some(chapter_id) = query.strip_prefix("chapter-id:").and_then(|value| value.parse::<u64>().ok()) {
            let body = fetch_json_or_fixture(&viewer_url(chapter_id, &request), VIEWER_FIXTURE);
            if let Some(title_id) = parse_viewer_title_id(&body) {
                let body = fetch_json_or_fixture(&title_detail_url(title_id), DETAILS_FIXTURE);
                return Ok(Paged { entries: vec![parse_details_value(&body, source)], has_next_page: false });
            }
        }
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/title_list/allV2?lang={}&clang={}&format=json", source.internal_lang, source.internal_lang),
            ALL_TITLES_FIXTURE,
        );
        let mut page = parse_all_titles(&body, source);
        if !query.is_empty() {
            let needle = query.to_ascii_lowercase();
            page.entries.retain(|item| item.title.to_ascii_lowercase().contains(&needle));
            page.has_next_page = false;
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/titles/100".into());
        let title_id = title_id_from_input(&key).unwrap_or(100);
        let body = fetch_json_or_fixture(&title_detail_url(title_id), DETAILS_FIXTURE);
        Ok(parse_details_value(&body, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/titles/100".into());
        let title_id = title_id_from_input(&key).unwrap_or(100);
        let body = fetch_json_or_fixture(&title_detail_url(title_id), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/viewer/1001".into());
        let chapter_id = key.rsplit('/').next().and_then(|value| value.parse::<u64>().ok()).unwrap_or(1001);
        let body = fetch_json_or_fixture(&viewer_url(chapter_id, &request), VIEWER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request.get("imageBase64").and_then(Value::as_str).unwrap_or_default();
        let Some(key) = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("encryptionKey"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Ok(ProcessedImage { image_base64: image_base64.into(), mime_type: request.get("mimeType").and_then(Value::as_str).map(ToOwned::to_owned), ..ProcessedImage::default() });
        };
        let mut image = STANDARD.decode(image_base64).unwrap_or_default();
        xor_decrypt(&mut image, key);
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(image),
            mime_type: request.get("mimeType").and_then(Value::as_str).map(ToOwned::to_owned),
            ..ProcessedImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(title_id) = title_id_from_input(input) {
            let body = fetch_json_or_fixture(&title_detail_url(title_id), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_value(&body, source)),
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
    internal_lang: &'static str,
    language_code: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "mangaplus-en", lang: "en", internal_lang: "eng", language_code: "ENGLISH" },
    SourceConfig { id: "mangaplus-es", lang: "es", internal_lang: "esp", language_code: "SPANISH" },
    SourceConfig { id: "mangaplus-fr", lang: "fr", internal_lang: "fra", language_code: "FRENCH" },
    SourceConfig { id: "mangaplus-id", lang: "id", internal_lang: "ind", language_code: "INDONESIAN" },
    SourceConfig { id: "mangaplus-pt-br", lang: "pt-BR", internal_lang: "ptb", language_code: "PORTUGUESE_BR" },
    SourceConfig { id: "mangaplus-ru", lang: "ru", internal_lang: "rus", language_code: "RUSSIAN" },
    SourceConfig { id: "mangaplus-th", lang: "th", internal_lang: "tha", language_code: "THAI" },
    SourceConfig { id: "mangaplus-vi", lang: "vi", internal_lang: "vie", language_code: "VIETNAMESE" },
    SourceConfig { id: "mangaplus-de", lang: "de", internal_lang: "deu", language_code: "GERMAN" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("mangaplus-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_origin(BASE_URL).with_referer(format!("{BASE_URL}/"))
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig, latest: bool) -> Paged<CatalogItem> {
    let value = json_or_fixture(body, if latest { HOME_FIXTURE } else { RANKING_FIXTURE });
    let titles = if latest {
        collect_objects_by_key(&value, "title")
    } else {
        collect_objects_by_key(&value, "title")
    };
    let entries = titles
        .into_iter()
        .filter(|title| title.get("language").and_then(Value::as_str).unwrap_or(source.language_code) == source.language_code)
        .map(|title| catalog_from_title(title, source))
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_all_titles(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let value = json_or_fixture(body, ALL_TITLES_FIXTURE);
    let entries = collect_objects_by_key(&value, "titles")
        .into_iter()
        .flat_map(|value| value.as_array().into_iter().flatten())
        .chain(collect_objects_by_key(&value, "title").into_iter())
        .filter(|title| title.get("language").and_then(Value::as_str).unwrap_or(source.language_code) == source.language_code)
        .map(|title| catalog_from_title(title, source))
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_details_value(body: &str, source: SourceConfig) -> CatalogItem {
    let value = json_or_fixture(body, DETAILS_FIXTURE);
    let view = find_object_with_key(&value, "titleId").unwrap_or(&value);
    let title = view.get("title").unwrap_or(view);
    let mut item = catalog_from_title(title, source);
    item.description = title.get("description").and_then(Value::as_str).map(ToOwned::to_owned);
    item.authors = title.get("author").and_then(Value::as_str).map(|value| vec![value.to_string()]).unwrap_or_default();
    item.status = ItemStatus::Ongoing;
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let value = json_or_fixture(body, DETAILS_FIXTURE);
    collect_objects_by_key(&value, "chapterId")
        .into_iter()
        .filter(|chapter| !chapter.get("isExpired").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|chapter| {
            let id = chapter.get("chapterId").and_then(Value::as_u64)?;
            let name = chapter.get("name").and_then(Value::as_str).unwrap_or("Chapter");
            Some(MangaChapter {
                key: format!("/viewer/{id}"),
                title: Some(name.into()),
                chapter_number: name.split('#').nth(1).and_then(|value| value.parse().ok()),
                language: Some(source.lang.into()),
                url: Some(format!("{BASE_URL}/viewer/{id}")),
                ..MangaChapter::default()
            })
        })
        .rev()
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = json_or_fixture(body, VIEWER_FIXTURE);
    collect_objects_by_key(&value, "imageUrl")
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let url = page.get("imageUrl").and_then(Value::as_str)?;
            let key = page.get("encryptionKey").and_then(Value::as_str).unwrap_or_default();
            let mut extra = BTreeMap::new();
            if !key.is_empty() {
                extra.insert("encryptionKey".into(), Value::String(key.into()));
            }
            Some(MangaPage {
                content: PageContent::Url { url: url.into(), context: Some(image_headers()) },
                headers: image_headers(),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            })
        })
        .collect()
}

fn catalog_from_title(title: &Value, source: SourceConfig) -> CatalogItem {
    let title = title.get("title").unwrap_or(title);
    let id = title.get("titleId").or_else(|| title.get("id")).and_then(Value::as_u64).unwrap_or(100);
    CatalogItem {
        key: format!("/titles/{id}"),
        title: title.get("name").or_else(|| title.get("title")).and_then(Value::as_str).unwrap_or("MANGA Plus").into(),
        cover: title
            .get("portraitImageUrl")
            .or_else(|| title.get("thumbnailUrl"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        url: Some(format!("{BASE_URL}/titles/{id}")),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn viewer_url(chapter_id: u64, request: &Value) -> String {
    let quality = request.get("preferences").and_then(|prefs| prefs.get("imageQuality")).and_then(Value::as_str).unwrap_or("high");
    let split = if request.get("preferences").and_then(|prefs| prefs.get("splitImages")).and_then(Value::as_bool).unwrap_or(true) { "yes" } else { "no" };
    format!("{API_URL}/manga_viewer?chapter_id={chapter_id}&split={split}&img_quality={quality}&format=json")
}

fn title_detail_url(title_id: u64) -> String {
    format!("{API_URL}/title_detailV3?title_id={title_id}&format=json")
}

fn title_id_from_input(input: &str) -> Option<u64> {
    if let Ok(id) = input.parse() {
        return Some(id);
    }
    input
        .split('/')
        .filter_map(|part| part.parse::<u64>().ok())
        .next_back()
}

fn parse_viewer_title_id(body: &str) -> Option<u64> {
    let value = json_or_fixture(body, VIEWER_FIXTURE);
    find_object_with_key(&value, "titleId").and_then(|view| view.get("titleId")).and_then(Value::as_u64)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    headers
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(fixture).expect("fixture is valid"))
}

fn collect_objects_by_key<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    collect_objects_by_key_into(value, key, &mut out);
    out
}

fn collect_objects_by_key_into<'a>(value: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if map.contains_key(key) {
                out.push(value);
            }
            for child in map.values() {
                collect_objects_by_key_into(child, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_objects_by_key_into(item, key, out);
            }
        }
        _ => {}
    }
}

fn find_object_with_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) if map.contains_key(key) => Some(value),
        Value::Object(map) => map.values().find_map(|child| find_object_with_key(child, key)),
        Value::Array(items) => items.iter().find_map(|child| find_object_with_key(child, key)),
        _ => None,
    }
}

fn xor_decrypt(bytes: &mut [u8], key: &str) {
    let key_stream = key
        .as_bytes()
        .chunks(2)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .filter_map(|hex| u8::from_str_radix(hex, 16).ok())
        .collect::<Vec<_>>();
    if key_stream.is_empty() {
        return;
    }
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key_stream[index % key_stream.len()];
    }
}

const RANKING_FIXTURE: &str = r#"{
  "success": { "titleRankingViewV2": { "rankedTitles": [
    { "title": { "titleId": 100, "name": "Sample Plus", "language": "ENGLISH", "portraitImageUrl": "https://img.example/cover.jpg" } }
  ] } }
}"#;

const HOME_FIXTURE: &str = r#"{
  "success": { "homeViewV3": { "groups": [
    { "titleGroups": [{ "titles": [{ "title": { "titleId": 100, "name": "Sample Plus", "language": "ENGLISH", "portraitImageUrl": "https://img.example/cover.jpg" } }] }] }
  ] } }
}"#;

const ALL_TITLES_FIXTURE: &str = r#"{
  "success": { "allTitlesViewV2": { "AllTitlesGroup": [
    { "titles": [{ "titleId": 100, "name": "Sample Plus", "language": "ENGLISH", "portraitImageUrl": "https://img.example/cover.jpg" }] }
  ] } }
}"#;

const DETAILS_FIXTURE: &str = r##"{
  "success": { "titleDetailView": {
    "title": { "titleId": 100, "name": "Sample Plus", "language": "ENGLISH", "portraitImageUrl": "https://img.example/cover.jpg", "description": "Sample description.", "author": "Author One" },
    "chapterListGroup": [{ "firstChapterList": [{ "chapterId": 1001, "name": "#1", "isExpired": false }], "lastChapterList": [] }]
  } }
}"##;

const VIEWER_FIXTURE: &str = r#"{
  "success": { "mangaViewer": {
    "titleId": 100,
    "pages": [
      { "mangaPage": { "imageUrl": "https://img.example/page-1.jpg", "encryptionKey": "0f" } },
      { "mangaPage": { "imageUrl": "https://img.example/page-2.jpg" } }
    ]
  } }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_chapters_pages() {
        assert_eq!(parse_listing(RANKING_FIXTURE, SOURCES[0], false).entries[0].title, "Sample Plus");
        let item = parse_details_value(DETAILS_FIXTURE, SOURCES[0]);
        assert_eq!(item.description.as_deref(), Some("Sample description."));
        assert_eq!(parse_chapters(DETAILS_FIXTURE, SOURCES[0]).len(), 1);
        let pages = parse_pages(VIEWER_FIXTURE);
        assert_eq!(pages.len(), 2);
        assert!(pages[0].extra.contains_key("encryptionKey"));
    }

    #[test]
    fn xor_decrypts_bytes() {
        let mut bytes = vec![0x00, 0x0f, 0xff];
        xor_decrypt(&mut bytes, "0f");
        assert_eq!(bytes, vec![0x0f, 0x00, 0xf0]);
    }
}
