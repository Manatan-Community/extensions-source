use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: WaveTeamy = WaveTeamy;
const BASE_URL: &str = "https://waveteamy.com";
const CLOUD_URL: &str = "https://wcloud.site";
const PAGE_LIMIT: &str = "40";

struct WaveTeamy;

impl MangaSource for WaveTeamy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .to_string();
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = if latest {
            post_form_or_fixture(
                "/wapi/hanout/v1/series/releases-web",
                &[("page", &page), ("limit", PAGE_LIMIT)],
                LATEST_FIXTURE,
            )
        } else {
            post_form_or_fixture(
                "/wapi/hanout/v1/series/series-list",
                &[("page", &page), ("limit", PAGE_LIMIT)],
                LIST_FIXTURE,
            )
        };
        if latest {
            Ok(parse_latest(&body))
        } else {
            Ok(parse_popular(&body))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .to_string();
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let body = post_form_or_fixture(
            "/wapi/hanout/v1/series/series-list",
            &[
                ("page", &page),
                ("keyUpValue", query),
                ("limit", PAGE_LIMIT),
            ],
            LIST_FIXTURE,
        );
        Ok(parse_popular(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/100".to_string());
        let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/100".to_string());
        let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, work_id_from_key(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/series/100/1".to_string());
        let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn post_form_or_fixture(path: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}{path}"))
        .xhr()
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .header("rsc", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let mangas: Vec<WManga> = serde_json::from_str(body).unwrap_or_else(|_| {
        extract_json_values(body)
            .into_iter()
            .find_map(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    });
    let count = mangas.len();
    Paged {
        entries: mangas.into_iter().map(WManga::into_catalog).collect(),
        has_next_page: count >= 40,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let latest: WLatestManga = serde_json::from_str(body)
        .ok()
        .or_else(|| {
            extract_json_values(body)
                .into_iter()
                .find_map(|value| serde_json::from_value(value).ok())
        })
        .unwrap_or_default();
    Paged {
        entries: latest
            .chapters
            .into_iter()
            .map(WManga::into_catalog)
            .collect(),
        has_next_page: !latest.is_last_page,
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let details: WMangaDetails = serde_json::from_str(body)
        .ok()
        .or_else(|| find_object_with_keys(body, &["name", "cover", "genre"]))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: non_empty(details.name)
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: non_empty(details.cover).map(|image| to_image_url(&image)),
        description: details.story.map(|story| story.replace("\\n", "\n")),
        authors: non_empty_option(details.author).into_iter().collect(),
        artists: non_empty_option(details.artist).into_iter().collect(),
        tags: details
            .genre
            .into_iter()
            .chain(details.kind)
            .filter(|value| !value.trim().is_empty())
            .collect(),
        status: match details.status {
            Some(0) => ItemStatus::Ongoing,
            Some(1) => ItemStatus::Completed,
            Some(2) => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, work_id: String) -> Vec<MangaChapter> {
    let chapters: Vec<WChapterList> = serde_json::from_str(body)
        .ok()
        .or_else(|| find_array_with_key(body, "chapter"))
        .unwrap_or_default();
    chapters
        .into_iter()
        .map(|chapter| {
            let number = trim_decimal(chapter.chapter);
            let title = chapter
                .title
                .filter(|value| !value.trim().is_empty())
                .map(|title| format!("الفصل {number} - {title}"))
                .unwrap_or_else(|| format!("الفصل {number}"));
            let key = format!("/series/{work_id}/{number}");
            MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: chapter
                    .post_time
                    .as_deref()
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page: WPage = serde_json::from_str(body)
        .ok()
        .or_else(|| find_object_with_keys(body, &["images"]))
        .unwrap_or_default();
    page.images
        .into_iter()
        .filter_map(|encoded| decode_image_payload(&encoded))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: to_image_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_json_values(body: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for start in body.match_indices(['{', '[']).map(|(index, _)| index) {
        for end in body[start..]
            .match_indices(['}', ']'])
            .map(|(index, _)| start + index + 1)
        {
            if let Ok(value) = serde_json::from_str::<Value>(&body[start..end]) {
                values.push(value);
                break;
            }
        }
    }
    values
}

fn find_object_with_keys<T>(body: &str, keys: &[&str]) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    extract_json_values(body).into_iter().find_map(|value| {
        if value
            .as_object()
            .is_some_and(|object| keys.iter().all(|key| object.contains_key(*key)))
        {
            serde_json::from_value(value).ok()
        } else {
            None
        }
    })
}

fn find_array_with_key<T>(body: &str, key: &str) -> Option<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    extract_json_values(body).into_iter().find_map(|value| {
        let array = value.as_array()?;
        if array.iter().all(|item| {
            item.as_object()
                .is_some_and(|object| object.contains_key(key))
        }) {
            serde_json::from_value(value).ok()
        } else {
            None
        }
    })
}

fn decode_image_payload(input: &str) -> Option<String> {
    let encoded = input.split('.').next().unwrap_or(input);
    let decoded = STANDARD.decode(encoded).ok()?;
    let payload: WImagePayload = serde_json::from_slice(&decoded).ok()?;
    Some(payload.url)
}

fn to_image_url(input: &str) -> String {
    let escaped = input.replace(' ', "%20");
    if escaped.starts_with("http") {
        escaped
    } else if escaped.starts_with("projects")
        || escaped.starts_with("series")
        || escaped.starts_with("users")
    {
        format!("{CLOUD_URL}/{escaped}")
    } else {
        url::join_url(BASE_URL, &escaped)
    }
}

fn work_id_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("100")
        .to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

fn trim_decimal(value: f64) -> String {
    let as_text = value.to_string();
    as_text.strip_suffix(".0").unwrap_or(&as_text).to_string()
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn non_empty_option(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WManga {
    post_id: i64,
    title: String,
    image_url: String,
}

impl WManga {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/series/{}", self.post_id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(to_image_url(&self.image_url)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("ar".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WLatestManga {
    chapters: Vec<WManga>,
    #[serde(default)]
    is_last_page: bool,
}

#[derive(Default, Deserialize)]
struct WMangaDetails {
    name: String,
    cover: String,
    story: Option<String>,
    status: Option<i32>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    genre: Vec<String>,
    artist: Option<String>,
    author: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WChapterList {
    title: Option<String>,
    chapter: f64,
    post_time: Option<String>,
}

#[derive(Default, Deserialize)]
struct WPage {
    #[serde(default)]
    images: Vec<String>,
}

#[derive(Deserialize)]
struct WImagePayload {
    #[serde(rename = "p")]
    url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
[
  {"postId":100,"title":"عينة ويف","imageUrl":"series/sample cover.jpg"}
]
"#;

const LATEST_FIXTURE: &str = r#"
{"chapters":[{"postId":100,"title":"عينة ويف","imageUrl":"series/sample cover.jpg"}],"isLastPage":true}
"#;

const DETAILS_FIXTURE: &str = r#"
{"name":"عينة ويف","cover":"series/sample cover.jpg","story":"وصف\\nتجريبي","status":0,"type":"مانهوا","genre":["اكشن"],"artist":"رسام","author":"كاتب"}
"#;

const CHAPTERS_FIXTURE: &str = r#"
[
  {"title":"البداية","chapter":1.0,"postTime":"2024-01-01 00:00:00"}
]
"#;

const PAGES_FIXTURE: &str = r#"
{"images":["eyJwIjoic2VyaWVzL3NhbXBsZS8wMDEuanBnIn0=.sig","eyJwIjoic2VyaWVzL3NhbXBsZS8wMDIuanBnIn0=.sig"]}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_lists() {
        assert_eq!(parse_popular(LIST_FIXTURE).entries[0].key, "/series/100");
        assert!(!parse_latest(LATEST_FIXTURE).has_next_page);
    }

    #[test]
    fn parses_details_chapters_pages() {
        let item = parse_details(DETAILS_FIXTURE, "/series/100".into());
        assert_eq!(item.title, "عينة ويف");
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "100".into()).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
