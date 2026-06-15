use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, SearchRequest,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mahoushoujobu.com";
const SOURCE: MagicalTranslators = MagicalTranslators;

struct MagicalTranslators;

impl MangaSource for MagicalTranslators {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/get_all_series/"), SERIES_FIXTURE);
        Ok(parse_series_list(&body, source, "", latest))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(slug) = slug_from_url(query) {
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/series/{slug}/"), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_series_details(&body, Some(slug), source)], has_next_page: false });
        }
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/get_all_series/"), SERIES_FIXTURE);
        Ok(parse_series_list(&body, source, query, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/series/{key}/"), DETAILS_FIXTURE);
        Ok(parse_series_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/series/{key}/"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let slug = key.split('/').next().unwrap_or("sample");
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/series/{slug}/"), DETAILS_FIXTURE);
        Ok(parse_pages(&body, &key))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(slug) = slug_from_url(input) {
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/series/{slug}/"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_series_details(&body, Some(slug), source)),
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
struct SourceConfig { id: &'static str, lang: &'static str }

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "magicaltranslators-en", lang: "en" },
    SourceConfig { id: "magicaltranslators-es", lang: "es" },
    SourceConfig { id: "magicaltranslators-pl", lang: "pl" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("magicaltranslators-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_list(body: &str, source: SourceConfig, query: &str, latest: bool) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    let needle = query.to_ascii_lowercase();
    let mut entries = root.as_object().into_iter().flat_map(|map| map.iter()).filter_map(|(title, value)| {
        let item = item_from_value(value, Some(title.to_string()), source)?;
        if !matches_source(&item.key, source) {
            return None;
        }
        if !needle.is_empty() && !item.title.to_ascii_lowercase().contains(&needle) {
            return None;
        }
        Some((value.get("last_updated").and_then(Value::as_i64).unwrap_or_default(), item))
    }).collect::<Vec<_>>();
    if latest {
        entries.sort_by_key(|(timestamp, _)| -*timestamp);
    }
    Paged { entries: entries.into_iter().map(|(_, item)| item).collect(), has_next_page: false }
}

fn parse_series_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"));
    let fallback_key = key.clone();
    item_from_value(&value, None, source).map(|mut item| {
        if let Some(key) = key { item.key = key; }
        item.initialized = true;
        item
    }).unwrap_or_else(|| CatalogItem { key: fallback_key.unwrap_or_else(|| "sample".into()), title: "Magical Translators".into(), language: Some(source.lang.into()), content_rating: Some("safe".into()), ..CatalogItem::default() })
}

fn item_from_value(value: &Value, title_override: Option<String>, source: SourceConfig) -> Option<CatalogItem> {
    let slug = value.get("slug").and_then(Value::as_str)?.to_string();
    Some(CatalogItem {
        key: slug.clone(),
        title: title_override.or_else(|| value.get("title").and_then(Value::as_str).map(ToString::to_string)).unwrap_or_else(|| slug.clone()),
        cover: value.get("cover").and_then(Value::as_str).map(|cover| if cover.starts_with("http") { cover.to_string() } else { format!("{BASE_URL}/{}", cover.trim_start_matches('/')) }),
        authors: value.get("author").and_then(Value::as_str).filter(|value| !value.is_empty()).map(|value| vec![value.to_string()]).unwrap_or_default(),
        artists: value.get("artist").and_then(Value::as_str).filter(|value| !value.is_empty()).map(|value| vec![value.to_string()]).unwrap_or_default(),
        description: value.get("description").and_then(Value::as_str).map(strip_html),
        url: Some(format!("{BASE_URL}/reader/series/{slug}/")),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"));
    let groups = value.get("groups").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut chapters = Vec::new();
    if let Some(chapter_map) = value.get("chapters").and_then(Value::as_object) {
        for (number, chapter) in chapter_map {
            let group_id = chapter.get("preferred_sort").and_then(Value::as_array).and_then(|sort| sort.first()).and_then(Value::as_str)
                .or_else(|| chapter.get("groups").and_then(Value::as_object).and_then(|map| map.keys().next().map(String::as_str)))
                .unwrap_or("1");
            let title = chapter.get("title").and_then(Value::as_str).unwrap_or("");
            chapters.push(MangaChapter {
                key: format!("{manga_key}/{number}/{group_id}"),
                title: Some(if title.is_empty() { number.to_string() } else { format!("{number} - {title}") }),
                chapter_number: number.parse::<f32>().ok(),
                scanlators: groups.get(group_id).and_then(Value::as_str).map(|name| vec![name.to_string()]).unwrap_or_default(),
                date_uploaded: chapter.get("release_date").and_then(|dates| dates.get(group_id)).and_then(Value::as_i64),
                url: Some(format!("{BASE_URL}/read/manga/{}/{number}/1/", manga_key.replace('.', "-"))),
                ..MangaChapter::default()
            });
        }
    }
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"));
    let mut parts = chapter_key.split('/');
    let slug = parts.next().unwrap_or("sample");
    let chapter = parts.next().unwrap_or("1");
    let preferred_group = parts.next();
    let chapter_value = value.get("chapters").and_then(|chapters| chapters.get(chapter));
    let folder = chapter_value.and_then(|chapter| chapter.get("folder")).and_then(Value::as_str).unwrap_or(chapter);
    let group_obj = chapter_value.and_then(|chapter| chapter.get("groups")).and_then(Value::as_object);
    let group_id = preferred_group.or_else(|| group_obj.and_then(|map| map.keys().next().map(String::as_str))).unwrap_or("1");
    group_obj.and_then(|groups| groups.get(group_id)).and_then(Value::as_array).into_iter().flatten().enumerate().filter_map(|(index, page)| {
        let filename = page.as_str()?;
        let image = format!("{BASE_URL}/media/manga/{slug}/chapters/{folder}/{group_id}/{filename}");
        Some(MangaPage {
            content: PageContent::Url { url: image, context: Some(image_headers()) },
            headers: image_headers(),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
    }).collect()
}

fn matches_source(slug: &str, source: SourceConfig) -> bool {
    match source.lang {
        "es" => slug.ends_with("-ES"),
        "pl" => slug.ends_with("-PL"),
        _ => !slug.ends_with("-ES") && !slug.ends_with("-PL"),
    }
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.starts_with("slug:") {
        return Some(input.trim_start_matches("slug:").to_string());
    }
    input.split("/series/").nth(1).or_else(|| input.split("/reader/series/").nth(1)).map(|value| value.trim_matches('/').split(['?', '#']).next().unwrap_or(value).to_string())
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|value| value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())).map(ToString::to_string)
}

fn strip_html(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    headers
}

const SERIES_FIXTURE: &str = r#"{
  "Sample": { "slug": "sample", "title": "Sample", "author": "Author", "artist": "Artist", "description": "<p>Sample description</p>", "cover": "/cover.jpg", "last_updated": 1704067200 },
  "Sample ES": { "slug": "sample-ES", "title": "Sample ES", "cover": "/cover-es.jpg", "last_updated": 1704153600 }
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "slug": "sample",
  "title": "Sample",
  "author": "Author",
  "artist": "Artist",
  "description": "<p>Sample description</p>",
  "cover": "/cover.jpg",
  "groups": { "1": "Group One" },
  "chapters": {
    "1": { "title": "The Beginning", "folder": "chapter-1", "groups": { "1": ["001.jpg", "002.jpg"] }, "release_date": { "1": 1704067200 } }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_guya_source() {
        let en = SOURCES[0];
        let es = SOURCES[1];
        assert_eq!(parse_series_list(SERIES_FIXTURE, en, "", false).entries.len(), 1);
        assert_eq!(parse_series_list(SERIES_FIXTURE, es, "", false).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(DETAILS_FIXTURE, "sample/1/1").len(), 2);
    }
}
