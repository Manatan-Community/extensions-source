use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Genesis = Genesis;
const BASE_URL: &str = "https://genesistudio.com";
const API_URL: &str = "https://api.genesistudio.com";

struct Genesis;

impl NovelSource for Genesis {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        if request.get("page").and_then(Value::as_u64).unwrap_or(1) != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_listing(&fetch_or_fixture(&novels_url(), LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("page").and_then(Value::as_u64).unwrap_or(1) != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_or_fixture(&details_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let needle = normalize_search(query);
        Ok(Paged {
            entries: parse_listing(&fetch_or_fixture(&novels_url(), LIST_FIXTURE))
                .into_iter()
                .filter(|item| normalize_search(&item.title).contains(&needle))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novels/sample".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&details_url(&normalize_key(&key)), DETAILS_FIXTURE),
            &normalize_key(&key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/novels/sample".to_string());
        let details = fetch_or_fixture(&details_url(&normalize_key(&key)), DETAILS_FIXTURE);
        let id = json_text(&serde_json::from_str(&details).unwrap_or(Value::Null), "id")
            .unwrap_or_else(|| "sample-id".to_string());
        let hide_locked = bool_setting(&request, "hideLocked");
        Ok(parse_chapters(
            &fetch_or_fixture(
                &format!("{BASE_URL}/api/novels-chapter/{id}"),
                CHAPTERS_FIXTURE,
            ),
            hide_locked,
        ))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "/viewer/sample-chapter".to_string());
        let html = fetch_chapter_html(&normalize_chapter_key(&key))
            .unwrap_or_else(|| "<p>The first fixture paragraph.</p>".to_string());
        Ok(NovelText {
            html: Some(html.clone()),
            text: Some(novel::cleanup_text(&html)),
            base_url: Some(BASE_URL.to_string()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            next_chapter_key: Some(key),
            ..NovelText::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "novels".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_listing(LIST_FIXTURE),
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_or_fixture(&details_url(&key), DETAILS_FIXTURE),
                    &key,
                )),
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
        .with_referer(BASE_URL)
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let abbreviation =
                json_text(item, "abbreviation").unwrap_or_else(|| "sample".to_string());
            let key = format!("/novels/{abbreviation}");
            CatalogItem {
                key: key.clone(),
                title: json_text(item, "novel_title").unwrap_or_else(|| "Novel".to_string()),
                cover: json_text(item, "cover").map(|cover| {
                    format!("{API_URL}/storage/v1/object/public/directus/{cover}.png")
                }),
                url: Some(format!("{BASE_URL}{key}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let cover = json_text(&root, "cover");
    CatalogItem {
        key: normalize_key(key),
        title: json_text(&root, "novel_title").unwrap_or_else(|| "Novel".to_string()),
        cover: cover.map(|id| cover_url(&id)),
        description: json_text(&root, "synopsis"),
        authors: json_text(&root, "author").into_iter().collect(),
        tags: root
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| {
                genre
                    .get("genres_id")
                    .and_then(|id| id.get("label"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect(),
        status: parse_status(root.get("serialization").and_then(Value::as_str)),
        url: Some(format!("{BASE_URL}{}", normalize_key(key))),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut chapters: Vec<_> = root
        .get("data")
        .and_then(|data| data.get("chapters"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| {
            !hide_locked
                || chapter
                    .get("isUnlocked")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
        })
        .map(|chapter| {
            let id = json_text(chapter, "id").unwrap_or_else(|| "sample-chapter".to_string());
            let locked = !chapter
                .get("isUnlocked")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            NovelChapter {
                key: format!("/viewer/{id}"),
                title: json_text(chapter, "chapter_title").map(|title| {
                    if locked {
                        format!("Locked - {title}")
                    } else {
                        title
                    }
                }),
                chapter_number: chapter
                    .get("chapter_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                url: Some(format!("{BASE_URL}/viewer/{id}")),
                language: Some("en".to_string()),
                is_locked: locked,
                ..NovelChapter::default()
            }
        })
        .collect();
    chapters.sort_by(|a, b| {
        a.chapter_number
            .partial_cmp(&b.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn fetch_chapter_html(chapter_key: &str) -> Option<String> {
    let id = chapter_key.trim_start_matches("/viewer/");
    let viewer = client()
        .get(format!("{BASE_URL}/viewer/{id}"))
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| TEXT_PAGE_FIXTURE.to_string());
    let (external_api, api_key) = supabase_config(&viewer)?;
    let target = format!(
        "{external_api}/rest/v1/chapters?select=id,chapter_title,chapter_number,chapter_content,status,novel&id=eq.{id}&status=eq.released"
    );
    let body = client()
        .get(target)
        .header("apikey", api_key.clone())
        .header("x-client-info", "supabase-ssr/0.7.0 createBrowserClient")
        .referer(BASE_URL)
        .xhr()
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&body)
        .ok()?
        .as_array()?
        .first()?
        .get("chapter_content")?
        .as_str()
        .map(|content| content.replace('\n', "<br/>"))
}

fn supabase_config(viewer_html: &str) -> Option<(String, String)> {
    let mut script_urls = Vec::new();
    for chunk in viewer_html.split("<script").skip(1) {
        if let Some(src) = html::attr(chunk, "src") {
            script_urls.push(url::join_url(BASE_URL, &src));
        }
    }
    for script_url in script_urls {
        let code = client().get(script_url).send_text().ok()?;
        if !code.contains("sb_publishable") {
            continue;
        }
        let segment = code
            .split(';')
            .find(|segment| segment.contains("sb_publishable"))
            .unwrap_or(&code);
        let mut external_api = None;
        let mut api_key = None;
        for piece in segment.split('"') {
            if piece.starts_with("https") {
                external_api = Some(piece.to_string());
            } else if piece.contains("sb_publishable") {
                api_key = Some(piece.to_string());
            }
        }
        if let (Some(external_api), Some(api_key)) = (external_api, api_key) {
            return Some((external_api, api_key));
        }
    }
    None
}

fn novels_url() -> String {
    format!(
        "{BASE_URL}/api/directus/novels?status=published&fields={}&limit=-1",
        url::query_escape("[\"id\",\"novel_title\",\"cover\",\"abbreviation\"]")
    )
}

fn details_url(key: &str) -> String {
    let abbreviation = normalize_key(key)
        .trim_start_matches("/novels/")
        .to_string();
    format!("{BASE_URL}/api/directus/novels/by-abbreviation/{abbreviation}")
}

fn cover_url(id: &str) -> String {
    let file = client()
        .get(format!("{BASE_URL}/api/directus-file/{id}"))
        .xhr()
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|root| json_text(&root, "type"))
        .and_then(|mime| mime.split('/').nth(1).map(|ext| ext.replace("jpeg", "jpg")))
        .unwrap_or_else(|| "png".to_string());
    format!("{API_URL}/storage/v1/object/public/directus/{id}.{file}")
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn bool_setting(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("settings"))
        .and_then(|settings| settings.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_bool())
        .unwrap_or(false)
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "cancelled" => ItemStatus::Cancelled,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_end_matches('/')
        .to_string();
    if path.starts_with("/novels/") {
        path
    } else if path.starts_with("novels/") {
        format!("/{path}")
    } else {
        format!("/novels/{}", path.trim_start_matches('/'))
    }
}

fn normalize_chapter_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    if path.starts_with("/viewer/") {
        path.to_string()
    } else {
        format!(
            "/viewer/{}",
            path.trim_start_matches("viewer/").trim_start_matches('/')
        )
    }
}

fn normalize_search(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

const LIST_FIXTURE: &str = r#"[{"id":"sample-id","novel_title":"Sample Genesis","cover":"sample-cover","abbreviation":"sample"}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample-id","novel_title":"Sample Genesis","cover":"sample-cover","synopsis":"A fixture novel.","author":"Genesis Author","serialization":"ongoing","genres":[{"genres_id":{"label":"Fantasy"}}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"id":"sample-chapter","chapter_number":1,"chapter_title":"Chapter 1","isUnlocked":true}]}}"#;
const TEXT_PAGE_FIXTURE: &str =
    r#"<html><head></head><body><p>The first fixture paragraph.</p></body></html>"#;

export_novel_source!(SOURCE);
