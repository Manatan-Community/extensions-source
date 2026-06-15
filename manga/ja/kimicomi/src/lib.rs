use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: KimiComi = KimiComi;
const BASE_URL: &str = "https://kimicomi.com";
const API_URL: &str = "https://kimicomi.com/api";
const SEARCH_PAGE_SIZE: u64 = 24;

struct KimiComi;

impl MangaSource for KimiComi {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_series_list(&fetch_document(
            &format!("{BASE_URL}/ranking/manga"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        if !query.is_empty() {
            return Ok(parse_search_json(
                &fetch_json(
                    &format!(
                        "{API_URL}/search?q={}&page={page}&size={SEARCH_PAGE_SIZE}",
                        url::query_escape(query)
                    ),
                    SEARCH_FIXTURE,
                ),
                page,
            ));
        }
        let path = filter_string(&request, "browse").unwrap_or("/series/list/up");
        let target = if path == "/ranking/manga" {
            format!("{BASE_URL}{path}")
        } else {
            format!("{BASE_URL}{path}/{page}")
        };
        Ok(parse_series_list(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let hash = key.trim_matches('/').split('/').last().unwrap_or("sample");
        let show_locked = preference_bool(&request, "showLockedChapters", true);
        let show_login = preference_bool(&request, "showLoginRequiredChapters", true);
        Ok(fetch_chapters(hash, show_locked, show_login))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        if key.contains("#login") {
            return Ok(vec![manga::text_page(
                "Log in via WebView to read this free chapter and refresh the entry.",
            )]);
        }
        let episode_id = key.trim_matches('/').split('/').last().unwrap_or("sample");
        let episode = fetch_json(&format!("{API_URL}/episodes/{episode_id}"), EPISODE_FIXTURE);
        let viewer_id = find_viewer_id(&episode).unwrap_or_else(|| "sample-viewer".into());
        Ok(fetch_viewer_pages(&viewer_id, API_URL))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::ComiciViewer::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| absolute_url(key.split('#').next().unwrap_or(&key))))
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("series-list-item") || chunk.contains("ranking-box"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "series-list-item-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-list-item-h", "</")
                    .or_else(|| html::text_between(chunk, "title-text", "</"))
                    .or_else(|| html::text_between(chunk, "<span", "</span>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "KimiComi".into())
                    }),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("g-pager-link mode-active") || body.contains("rel=\"next\""),
    }
}

fn parse_search_json(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or(Value::Null));
    let result = root.pointer("/searchResult/series");
    let total = result
        .and_then(|v| v.get("total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entries = result
        .and_then(|v| v.get("series"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            Some(CatalogItem {
                key: format!("/series/{id}"),
                title: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("KimiComi")
                    .to_string(),
                cover: first_image(item),
                url: Some(format!("{BASE_URL}/series/{id}")),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: total > page * SEARCH_PAGE_SIZE,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let hash = key.trim_matches('/').split('/').last().unwrap_or("sample");
    let body = fetch_json(
        &format!("{API_URL}/episodes?seriesHash={hash}"),
        DETAILS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap_or(Value::Null));
    let summary = root.pointer("/series/summary").unwrap_or(&Value::Null);
    CatalogItem {
        key: format!("/series/{hash}"),
        title: summary
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("KimiComi")
            .to_string(),
        cover: first_image(summary),
        authors: summary
            .get("author")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        description: summary
            .get("description")
            .and_then(Value::as_str)
            .map(parse_description)
            .filter(|value| !value.is_empty()),
        tags: summary
            .get("tag")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        status: if summary
            .get("isCompleted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/series/{hash}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(hash: &str, show_locked: bool, show_login: bool) -> Vec<MangaChapter> {
    let details = fetch_json(
        &format!("{API_URL}/episodes?seriesHash={hash}&episodeFrom=1&episodeTo=9999"),
        CHAPTERS_FIXTURE,
    );
    let access = fetch_json(
        &format!("{API_URL}/series/access?seriesHash={hash}&episodeFrom=1&episodeTo=9999"),
        ACCESS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&details)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or(Value::Null));
    let access_root = serde_json::from_str::<Value>(&access)
        .unwrap_or_else(|_| serde_json::from_str(ACCESS_FIXTURE).unwrap_or(Value::Null));
    let access_items = access_root
        .pointer("/seriesAccess/episodeAccesses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut chapters = root
        .pointer("/series/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let id = episode.get("id").and_then(Value::as_str)?;
            let access = access_items
                .iter()
                .find(|item| item.get("episodeId").and_then(Value::as_str) == Some(id));
            let has_access = access
                .and_then(|item| item.get("hasAccess"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let campaign = access
                .and_then(|item| item.get("isCampaign"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let locked = !has_access;
            if locked && campaign && !show_login {
                return None;
            }
            if locked && !campaign && !show_locked {
                return None;
            }
            let suffix = if locked && campaign { "#login" } else { "" };
            Some(MangaChapter {
                key: format!("/episodes/{id}{suffix}"),
                title: Some(format!(
                    "{}{}",
                    if locked && campaign {
                        "Login "
                    } else if locked {
                        "Locked "
                    } else {
                        ""
                    },
                    episode
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Chapter")
                )),
                date_uploaded: episode
                    .get("datePublished")
                    .and_then(Value::as_i64)
                    .map(|value| value * 1000),
                url: Some(format!("{BASE_URL}/episodes/{id}")),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn fetch_viewer_pages(viewer_id: &str, api_url: &str) -> Vec<MangaPage> {
    let first = fetch_json(
        &format!(
            "{api_url}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id=&page-from=0&page-to=1"
        ),
        VIEWER_FIXTURE,
    );
    let total = serde_json::from_str::<Value>(&first)
        .ok()
        .and_then(|value| value.get("totalPages").and_then(Value::as_u64))
        .unwrap_or(1);
    let body = fetch_json(
        &format!(
            "{api_url}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id=&page-from=0&page-to={total}"
        ),
        VIEWER_FIXTURE,
    );
    parse_viewer_pages(&body)
}

fn parse_viewer_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(VIEWER_FIXTURE).unwrap_or(Value::Null));
    root.get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let image = item.get("imageUrl").and_then(Value::as_str)?;
            let scramble = item
                .get("scramble")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut extra = BTreeMap::new();
            if !scramble.is_empty() {
                extra.insert("comiciScramble".into(), Value::String(scramble.to_string()));
            }
            Some(MangaPage {
                content: PageContent::Url {
                    url: image.to_string(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!(
                    "Page {}",
                    item.get("sort").and_then(Value::as_u64).unwrap_or(0) + 1
                )),
                extra,
                ..MangaPage::default()
            })
        })
        .collect()
}

fn find_viewer_id(body: &str) -> Option<String> {
    let root = serde_json::from_str::<Value>(body).ok()?;
    root.pointer("/episode/content")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("viewer"))
        .and_then(|item| item.get("viewerId").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn parse_description(input: &str) -> String {
    if let Ok(Value::Array(nodes)) = serde_json::from_str::<Value>(input) {
        return nodes
            .iter()
            .map(description_node_text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    input.to_string()
}

fn description_node_text(node: &Value) -> String {
    if let Some(text) = node.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(description_node_text)
        .collect::<Vec<_>>()
        .join("")
}

fn first_image(value: &Value) -> Option<String> {
    value
        .get("images")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "series-list-item-img", "src")
        .or_else(|| {
            html::attr_after(chunk, "<source", "data-srcset").and_then(|value| {
                value
                    .split_whitespace()
                    .next()
                    .map(|v| v.trim_start_matches("//").to_string())
            })
        })
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| {
            if value.starts_with("http") {
                value
            } else {
                absolute_url(&value)
            }
        })
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)?
        .get(id)?
        .as_str()
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(default)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/series/") {
        Some(normalize_key(input))
    } else if input.starts_with("/series/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="series-list-item"><a class="series-list-item-link" href="/series/sample"><img class="series-list-item-img" src="/cover.jpg"><div class="series-list-item-h"><span>Sample KimiComi</span></div></a></div>"#;
const SEARCH_FIXTURE: &str = r#"{"searchResult":{"series":{"total":1,"series":[{"id":"sample","name":"Sample KimiComi","images":[{"url":"https://img.example.test/cover.jpg"}]}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"summary":{"name":"Sample KimiComi","description":"Sample description.","author":[{"name":"Sample Author"}],"images":[{"url":"https://img.example.test/cover.jpg"}],"tag":[{"name":"Action"}],"isCompleted":false},"episodes":[{"id":"sample","title":"Episode 1","datePublished":1704067200}]}}"#;
const CHAPTERS_FIXTURE: &str = DETAILS_FIXTURE;
const ACCESS_FIXTURE: &str = r#"{"seriesAccess":{"episodeAccesses":[{"episodeId":"sample","hasAccess":true,"isCampaign":false}]}}"#;
const EPISODE_FIXTURE: &str =
    r#"{"episode":{"content":[{"type":"viewer","viewerId":"sample-viewer"}]}}"#;
const VIEWER_FIXTURE: &str = r#"{"totalPages":1,"result":[{"imageUrl":"https://img.example.test/page1.jpg","scramble":"","sort":0}]}"#;
