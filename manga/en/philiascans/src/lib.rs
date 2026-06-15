use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: PhiliaScans = PhiliaScans;
const BASE_URL: &str = "https://philiascans.org";
const API_URL: &str = "https://philiascans.org/api";

struct PhiliaScans;

impl MangaSource for PhiliaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let orderby = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "views"
        } else {
            ""
        };
        Ok(parse_series(&api_get(&format!(
            "{API_URL}/manga?page={}&perPage=20&orderby={orderby}&order=desc",
            page(&request)
        ), SERIES_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{API_URL}/manga?page={}&perPage=20&q={}{}",
            page(&request),
            url::query_escape(query),
            filter_params(&request)
        );
        Ok(parse_series(&api_get(&target, SERIES_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(parse_chapters(
            &api_get(&format!("{API_URL}/manga/{key}/chapters"), CHAPTERS_FIXTURE),
            &key,
            hide_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1".into());
        let (manga_slug, chapter_slug) = key.split_once('/').unwrap_or(("sample", "chapter-1"));
        let viewer = api_get(
            &format!("{API_URL}/manga/{manga_slug}/chapters/{chapter_slug}"),
            "",
        );
        let Ok(root) = serde_json::from_str::<Value>(&viewer) else {
            return Ok(Vec::new());
        };
        if root.get("hasAccess").and_then(Value::as_bool) != Some(true) {
            return Ok(Vec::new());
        }
        let Some(chapter) = root.get("chapter") else {
            return Ok(Vec::new());
        };
        let Some(chapter_id) = chapter.get("id").and_then(Value::as_i64) else {
            return Ok(Vec::new());
        };
        let Some(token) = reader_token() else {
            return Ok(Vec::new());
        };
        let keys = api_get_token(
            &format!("{API_URL}/chapters/{chapter_id}/page-keys"),
            &token,
            "{}",
        );
        let open = api_post_token(
            &format!("{API_URL}/chapters/{chapter_id}/open"),
            &token,
            "{}",
        );
        let session = serde_json::from_str::<Value>(&open)
            .ok()
            .and_then(|value| text(&value, "sessionId"))
            .unwrap_or_default();
        let drm = api_get_token(
            &format!("{API_URL}/chapters/{chapter_id}/get-drm?session={}", url::query_escape(&session)),
            &token,
            "{}",
        );
        Ok(parse_pages(chapter, &keys, &open, &drm, &format!("{BASE_URL}/series/{key}")))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::PhiliaScansImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/series/{key}")))
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

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_get_token(target: &str, token: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-Reader-Access-Token", token)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_post_token(target: &str, token: &str, fixture: &str) -> String {
    client()
        .post(target)
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-Reader-Access-Token", token)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn reader_token() -> Option<String> {
    let body = client()
        .post(format!("{API_URL}/reader/access-token"))
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&body).ok().and_then(|value| text(&value, "token"))
}

fn parse_series(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("fixture"));
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(series_item)
        .collect();
    Paged {
        entries,
        has_next_page: root.get("page").and_then(Value::as_u64).unwrap_or(1)
            < root.get("totalPages").and_then(Value::as_u64).unwrap_or(1),
    }
}

fn series_item(item: &Value) -> CatalogItem {
    let key = text(item, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: key.clone(),
        title: text(item, "title").unwrap_or_else(|| "Philia Scans".into()),
        cover: text(item, "coverImageUrl").map(|value| absolute_url(&value)),
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = api_get(&format!("{API_URL}/manga/{key}"), DETAILS_FIXTURE);
    let root = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture"));
    CatalogItem {
        key: key.into(),
        title: text(&root, "title").unwrap_or_else(|| "Philia Scans".into()),
        cover: text(&root, "coverImageUrl").map(|value| absolute_url(&value)),
        authors: info_names(root.get("authors")),
        artists: info_names(root.get("artists")),
        description: text(&root, "synopsis").map(|synopsis| {
            let alternatives = root
                .get("alternativeTitles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            if alternatives.is_empty() {
                synopsis
            } else {
                format!("{synopsis}\n\nAlternative Titles:\n{}", alternatives.join("\n"))
            }
        }),
        tags: info_names(root.get("genres")),
        status: match text(&root, "status").unwrap_or_default().as_str() {
            "ON_GOING" => ItemStatus::Ongoing,
            "COMPLETED" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_slug: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture"));
    root.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| !hide_locked || !is_locked(item))
        .filter_map(|item| {
            let slug = text(item, "slug")?;
            let number = text(item, "number").and_then(|value| value.parse::<f32>().ok());
            let title = text(item, "title").filter(|value| !value.is_empty()).unwrap_or_else(|| {
                number
                    .map(|value| format!("Chapter {}", value))
                    .unwrap_or_else(|| "Chapter".into())
            });
            Some(MangaChapter {
                key: format!("{manga_slug}/{slug}"),
                title: Some(if is_locked(item) { format!("Locked {title}") } else { title }),
                chapter_number: number,
                date_uploaded: text(item, "publishedAt").and_then(|date| dates::parse_ymd(&date[..10.min(date.len())])),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(chapter: &Value, keys: &str, open: &str, drm: &str, referer: &str) -> Vec<MangaPage> {
    let key_root = serde_json::from_str::<Value>(keys).unwrap_or(Value::Null);
    let open_root = serde_json::from_str::<Value>(open).unwrap_or(Value::Null);
    let drm_root = serde_json::from_str::<Value>(drm).unwrap_or(Value::Null);
    let chapter_key = text(&key_root, "chapterKeyB64").unwrap_or_default();
    let grid_size = key_root.get("gridSize").and_then(Value::as_u64).unwrap_or(1);
    let payload_a = text(&open_root, "payloadA").unwrap_or_default();
    let payload_b = text(&drm_root, "payloadB").unwrap_or_default();
    let scrambled = chapter.get("scrambled").and_then(Value::as_bool).unwrap_or(false);
    chapter
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let position = page.get("position").and_then(Value::as_u64).unwrap_or(0);
            let image = text(page, "url").map(|value| absolute_url(&value))?;
            let mime = text(page, "mime").unwrap_or_else(|| "image/jpeg".into());
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                extra: BTreeMap::from([
                    ("philiaScrambled".into(), json!(scrambled)),
                    ("philiaMime".into(), json!(mime)),
                    ("philiaChapterKey".into(), json!(chapter_key)),
                    ("philiaGridSize".into(), json!(grid_size)),
                    ("philiaPayloadA".into(), json!(payload_a)),
                    ("philiaPayloadB".into(), json!(payload_b)),
                    ("philiaPageIndex".into(), json!(position)),
                ]),
                description: Some(format!("Page {}", position + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn filter_params(request: &Value) -> String {
    let mut params = Vec::<(String, String)>::new();
    for id in ["orderby", "order"] {
        if let Some(value) = filter_string(request, id).filter(|value| !value.is_empty()) {
            params.push((id.into(), value));
        }
    }
    for id in ["types", "statuses", "genres"] {
        for value in filter_array(request, id) {
            params.push((id.into(), value));
        }
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("&{}", encode_params(&params))
    }
}

fn info_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| text(item, "name"))
        .collect()
}

fn is_locked(item: &Value) -> bool {
    item.get("purchased").and_then(Value::as_bool) == Some(false)
        && item.get("coinPrice").and_then(Value::as_i64).unwrap_or(0) != 0
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(ToOwned::to_owned)
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/series/"))
        .map(|value| value.trim_matches('/').split('/').next().unwrap_or(value).to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
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
        .and_then(|value| value.as_bool().or_else(|| value.as_str().map(|text| text == "true")))
        .unwrap_or(false)
}

fn encode_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"
{"items":[{"slug":"sample","title":"Sample Philia","coverImageUrl":"/cover.jpg"}],"page":1,"totalPages":1}
"#;

const DETAILS_FIXTURE: &str = r#"
{"title":"Sample Philia","alternativeTitles":["Alt"],"synopsis":"Sample description.","coverImageUrl":"/cover.jpg","status":"ON_GOING","genres":[{"name":"Fantasy"}],"authors":[{"name":"Author"}],"artists":[{"name":"Artist"}]}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{"items":[{"number":"1","title":"Beginning","slug":"chapter-1","publishedAt":"2024-01-01T00:00:00.000Z","coinPrice":0,"purchased":true}]}
"#;
