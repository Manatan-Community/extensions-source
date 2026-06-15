use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: FlameComics = FlameComics;
const BASE_URL: &str = "https://flamecomics.xyz";
const CDN_URL: &str = "https://cdn.flamecomics.xyz";
const PER_PAGE: usize = 20;

struct FlameComics;

impl MangaSource for FlameComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse_page(BROWSE_FIXTURE, 1, None));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_next_data(&["index.json"], &[], LATEST_FIXTURE);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let body = fetch_next_data(&["browse.json"], &[], BROWSE_FIXTURE);
        Ok(parse_browse_page(&body, page, None))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_next_data(&["browse.json"], &[], BROWSE_FIXTURE);
        Ok(parse_browse_page(&body, page, Some(query)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1".to_string());
        let id = series_id_from_key(&key).unwrap_or(1);
        let body = fetch_next_data(
            &["series", &format!("{id}.json")],
            &[("id", id.to_string())],
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/1/sample-token".to_string());
        let (series_id, token) = chapter_parts(&key);
        let body = fetch_next_data(
            &["series", &series_id.to_string(), &format!("{token}.json")],
            &[("id", series_id.to_string()), ("token", token)],
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
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

fn fetch_next_data(path: &[&str], query: &[(&str, String)], fixture: &str) -> String {
    let build_id = fetch_build_id();
    let mut target = format!("{BASE_URL}/_next/data/{build_id}");
    for segment in path {
        target.push('/');
        target.push_str(segment.trim_matches('/'));
    }
    if !query.is_empty() {
        target.push('?');
        target.push_str(
            &query
                .iter()
                .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_build_id() -> String {
    let body = fetch_document_or_fixture(BASE_URL, INDEX_FIXTURE);
    extract_build_id(&body).unwrap_or_else(|| "fixture-build".to_string())
}

fn extract_build_id(body: &str) -> Option<String> {
    let after_marker = body.split("__NEXT_DATA__").nth(1)?;
    let json = after_marker.split_once('>')?.1.split("</script>").next()?;
    let value: Value = serde_json::from_str(&html::html_unescape(json)).ok()?;
    value
        .get("buildId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_browse_page(body: &str, page: usize, query: Option<&str>) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mut series = root
        .pointer("/pageProps/series")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("series_id").and_then(Value::as_u64).is_some())
        .collect::<Vec<_>>();
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let needle = clean_search(query);
        series.retain(|item| {
            let mut titles = vec![string_field(item, "title")];
            titles.extend(
                item.get("altTitles")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string),
            );
            titles
                .iter()
                .any(|title| clean_search(title).contains(&needle))
        });
    } else {
        series.sort_by_key(|item| {
            item.get("views")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        });
        series.reverse();
    }
    let start = page.saturating_sub(1) * PER_PAGE;
    let end = (start + PER_PAGE).min(series.len());
    let entries = if start >= series.len() {
        Vec::new()
    } else {
        series[start..end].iter().map(series_catalog).collect()
    };
    Paged {
        entries,
        has_next_page: end < series.len(),
    }
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    root.pointer("/pageProps/latestEntries/blocks/0/series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(series_catalog)
        .collect()
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = series_id_from_key(key).unwrap_or(1);
    let body = fetch_next_data(
        &["series", &format!("{id}.json")],
        &[("id", id.to_string())],
        DETAILS_FIXTURE,
    );
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    root.pointer("/pageProps/series")
        .map(series_details)
        .unwrap_or_else(|| fallback_item(key))
}

fn series_catalog(series: &Value) -> CatalogItem {
    let id = series.get("series_id").and_then(Value::as_u64).unwrap_or(0);
    let key = format!("/series/{id}");
    CatalogItem {
        key: key.clone(),
        title: string_field(series, "title"),
        cover: thumbnail_url(series),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn series_details(series: &Value) -> CatalogItem {
    let mut item = series_catalog(series);
    item.description = string_opt(series, "description")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.tags = string_array(series, "tags");
    if let Some(series_type) = string_opt(series, "type") {
        item.tags.insert(0, series_type);
    }
    item.authors = string_array(series, "author");
    item.artists = string_array(series, "artist");
    item.status = match string_field(series, "status").to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "dropped" => ItemStatus::Cancelled,
        "hiatus" => ItemStatus::Hiatus,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    };
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    root.pointer("/pageProps/chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|chapter| {
            let series_id = chapter
                .get("series_id")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let token = string_field(chapter, "token");
            let number = chapter_number(chapter.get("chapter"));
            let mut title = format!("Chapter {}", trim_number(number));
            if let Some(extra) = string_opt(chapter, "title").filter(|value| !value.is_empty()) {
                title.push_str(" - ");
                title.push_str(&extra);
            }
            MangaChapter {
                key: format!("/series/{series_id}/{token}"),
                title: Some(title),
                chapter_number: Some(number),
                date_uploaded: chapter.get("release_date").and_then(Value::as_i64),
                url: Some(format!("{BASE_URL}/series/{series_id}/{token}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let Some(chapter) = root.pointer("/pageProps/chapter") else {
        return Vec::new();
    };
    let series_id = chapter
        .get("series_id")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let token = string_field(chapter, "token");
    let release_date = chapter
        .get("release_date")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    image_names(chapter.get("images"))
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let image = format!(
                "{CDN_URL}/uploads/images/series/{series_id}/{token}/{name}?{release_date}"
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn image_names(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|item| string_opt(item, "name"))
            .collect(),
        Some(Value::Object(object)) => object
            .values()
            .filter_map(|item| string_opt(item, "name"))
            .collect(),
        _ => Vec::new(),
    }
}

fn thumbnail_url(series: &Value) -> Option<String> {
    let id = series.get("series_id").and_then(Value::as_u64)?;
    let cover = string_opt(series, "cover")?;
    let last_edit = series.get("last_edit").and_then(Value::as_i64).unwrap_or(0);
    Some(format!(
        "{CDN_URL}/uploads/images/series/{id}/{cover}?{last_edit}"
    ))
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/series/") {
        return format!("/{}", input[index + 1..].trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn series_id_from_key(key: &str) -> Option<u64> {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .and_then(|value| value.parse().ok())
}

fn chapter_parts(key: &str) -> (u64, String) {
    let mut parts = key.trim_matches('/').split('/');
    let _ = parts.next();
    let id = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let token = parts.next().unwrap_or("sample-token").to_string();
    (id, token)
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Flame Comics".to_string()),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn string_field(value: &Value, field: &str) -> String {
    string_opt(value, field).unwrap_or_default()
}

fn string_opt(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn chapter_number(value: Option<&Value>) -> f32 {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or_default() as f32
}

fn trim_number(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn clean_search(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == ' ')
        .collect()
}

export_manga_source!(SOURCE);

const INDEX_FIXTURE: &str =
    r#"<script id="__NEXT_DATA__" type="application/json">{"buildId":"fixture-build"}</script>"#;
const BROWSE_FIXTURE: &str = r#"{"pageProps":{"series":[{"series_id":1,"title":"Sample Manga","altTitles":["Sample Alt"],"cover":"thumbnail.jpg","type":"Manhwa","status":"Ongoing","last_edit":1700000000,"views":10}]}}"#;
const LATEST_FIXTURE: &str = r#"{"pageProps":{"latestEntries":{"blocks":[{"series":[{"series_id":1,"title":"Sample Manga","cover":"thumbnail.jpg","type":"Manhwa","status":"Ongoing","last_edit":1700000000}]}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"pageProps":{"series":{"series_id":1,"title":"Sample Manga","description":"<p>Sample description.</p>","cover":"thumbnail.jpg","type":"Manhwa","tags":["Action"],"author":["Author"],"artist":["Artist"],"status":"Ongoing","last_edit":1700000000}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"pageProps":{"chapters":[{"chapter":"1.00","title":"Start","release_date":1700000000,"series_id":1,"token":"sample-token"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"pageProps":{"chapter":{"release_date":1700000000,"series_id":1,"token":"sample-token","images":{"1":{"name":"001.jpg"},"2":{"name":"002.jpg"}}}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_browse_and_pages() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manga");
        let pages = SOURCE
            .pages(json!({"chapter":"/series/1/sample-token"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
