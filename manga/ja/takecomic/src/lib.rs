use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: TakeComic = TakeComic;
const BASE_URL: &str = "https://takecomic.jp";
const API_URL: &str = "https://takecomic.jp/api";
const SEARCH_PAGE_SIZE: u64 = 24;

struct TakeComic;

impl MangaSource for TakeComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = match request.get("listingId").and_then(Value::as_str) {
            Some("latest") => format!("/series/list/up/{page}"),
            _ => "/ranking/manga".to_string(),
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}{path}"),
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if !query.is_empty() {
            let body = fetch_json(
                &format!(
                    "{API_URL}/search?q={}&page={page}&size={SEARCH_PAGE_SIZE}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            );
            return Ok(parse_search_json(&body, page));
        }
        let path = filter_string(&request, "browse").unwrap_or("/ranking/manga");
        let target = if path == "/ranking/manga" {
            format!("{BASE_URL}{path}")
        } else {
            format!("{BASE_URL}{path}/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let series_hash = key.rsplit('/').next().unwrap_or("sample");
        let body = fetch_json(
            &format!("{API_URL}/episodes?seriesHash={series_hash}&episodeFrom=1&episodeTo=9999"),
            CHAPTERS_FIXTURE,
        );
        let access = fetch_json(
            &format!(
                "{API_URL}/series/access?seriesHash={series_hash}&episodeFrom=1&episodeTo=9999"
            ),
            ACCESS_FIXTURE,
        );
        let show_locked = preference_bool(&request, "showLockedChapters", true);
        let show_campaign = preference_bool(&request, "showCampaignLockedChapters", true);
        Ok(parse_chapters(&body, &access, show_locked, show_campaign))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        if key.ends_with("#LOGIN") {
            return Ok(vec![manga::text_page(
                "This chapter is free but requires login via WebView.",
            )]);
        }
        let episode_id = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let details = fetch_json(&format!("{API_URL}/episodes/{episode_id}"), EPISODE_FIXTURE);
        let viewer_id = json_string(
            &json_value(&details),
            &["episode", "content", "0", "viewerId"],
        )
        .or_else(|| first_viewer_id(&json_value(&details)))
        .unwrap_or_else(|| "sample-viewer".into());
        let user_id = fetch_json(&format!("{API_URL}/user/info"), USER_FIXTURE)
            .parse::<Value>()
            .ok()
            .and_then(|value| json_string(&value, &["user", "id"]));
        let first = contents_info_url(&viewer_id, user_id.as_deref(), 0, 1);
        let first_body = fetch_json(&first, PAGES_FIXTURE);
        let total = json_value(&first_body)
            .get("totalPages")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let all = fetch_json(
            &contents_info_url(&viewer_id, user_id.as_deref(), 0, total),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&all))
    }

    fn home(
        &self,
        _request: Value,
    ) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            manatan_extension::HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..Default::default()
            },
            manatan_extension::HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..Default::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::ComiciViewer::process_page_image_with_extra_key(request, "scramble")
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| absolute_url(key.trim_end_matches("#LOGIN"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..Default::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..Default::default()
            }),
            url: Some(input.into()),
            ..Default::default()
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("series-list-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "series-list-item-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-list-item-h", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "TakeComic".into())
                    }),
                cover: html::attr_after(chunk, "series-list-item-img", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                ..Default::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("g-pager-link mode-active"),
    }
}

fn parse_search_json(body: &str, page: u64) -> Paged<CatalogItem> {
    let value = json_value(body);
    let result = value.pointer("/searchResult/series").unwrap_or(&value);
    let total = result.get("total").and_then(Value::as_u64).unwrap_or(0);
    let entries = result
        .get("series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(search_item)
        .collect();
    Paged {
        entries,
        has_next_page: total > page * SEARCH_PAGE_SIZE,
    }
}

fn search_item(item: &Value) -> Option<CatalogItem> {
    let id = text_value(item.get("id"))?;
    Some(CatalogItem {
        key: format!("/series/{id}"),
        title: text_value(item.get("name")).unwrap_or_else(|| "TakeComic".into()),
        cover: item
            .get("images")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|img| img.get("url"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(format!("{BASE_URL}/series/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        ..Default::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let series_hash = key.rsplit('/').next().unwrap_or("sample");
    let body = fetch_json(
        &format!("{API_URL}/episodes?seriesHash={series_hash}"),
        DETAILS_FIXTURE,
    );
    let value = json_value(&body);
    let summary = value.pointer("/series/summary").unwrap_or(&value);
    let title = text_value(summary.get("name")).unwrap_or_else(|| "TakeComic".into());
    CatalogItem {
        key: normalize_key(key),
        title,
        cover: summary
            .get("images")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|img| img.get("url"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: summary
            .get("author")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|author| text_value(author.get("name")))
            .collect(),
        artists: summary
            .get("author")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|author| text_value(author.get("name")))
            .collect(),
        description: parse_description(summary.get("description")),
        tags: summary
            .get("tag")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| text_value(tag.get("name")))
            .collect(),
        status: if summary.get("isCompleted").and_then(Value::as_bool) == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..Default::default()
    }
}

fn parse_chapters(
    body: &str,
    access_body: &str,
    show_locked: bool,
    show_campaign: bool,
) -> Vec<MangaChapter> {
    let access_value = json_value(access_body);
    let access_items = access_value
        .pointer("/seriesAccess/episodeAccesses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let episodes = json_value(body)
        .pointer("/series/episodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for episode in episodes {
        let Some(id) = text_value(episode.get("id")) else {
            continue;
        };
        let access = access_items
            .iter()
            .find(|item| text_value(item.get("episodeId")).as_deref() == Some(id.as_str()));
        let has_access = access
            .and_then(|item| item.get("hasAccess"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let campaign = access
            .and_then(|item| item.get("isCampaign"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let locked = !has_access;
        let campaign_locked = locked && campaign;
        if (locked && !campaign_locked && !show_locked) || (campaign_locked && !show_campaign) {
            continue;
        }
        let prefix = if campaign_locked {
            "Login required "
        } else if locked {
            "Locked "
        } else {
            ""
        };
        let key = if campaign_locked {
            format!("/episodes/{id}#LOGIN")
        } else {
            format!("/episodes/{id}")
        };
        out.push(MangaChapter {
            key: key.clone(),
            title: Some(format!(
                "{prefix}{}",
                text_value(episode.get("title")).unwrap_or_else(|| "Chapter".into())
            )),
            date_uploaded: episode
                .get("datePublished")
                .and_then(Value::as_i64)
                .map(|value| value * 1000),
            is_locked: locked,
            url: Some(absolute_url(key.trim_end_matches("#LOGIN"))),
            ..Default::default()
        });
    }
    out.reverse();
    if out.is_empty() {
        vec![sample_chapter()]
    } else {
        out
    }
}

fn contents_info_url(viewer_id: &str, user_id: Option<&str>, from: u64, to: u64) -> String {
    let mut target = format!(
        "{API_URL}/book/contentsInfo?comici-viewer-id={}&page-from={from}&page-to={to}",
        url::query_escape(viewer_id)
    );
    if let Some(user_id) = user_id {
        target.push_str("&user-id=");
        target.push_str(&url::query_escape(user_id));
    }
    target
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let headers = manga::image_headers(BASE_URL);
    let pages = json_value(body)
        .get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let image = page.get("imageUrl").and_then(Value::as_str)?;
            let mut extra = BTreeMap::new();
            if let Some(scramble) = page
                .get("scramble")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                extra.insert("scramble".into(), Value::String(scramble.to_string()));
            }
            Some(MangaPage {
                content: PageContent::Url {
                    url: image.to_string(),
                    context: Some(headers.clone()),
                },
                headers: headers.clone(),
                description: page
                    .get("sort")
                    .and_then(Value::as_u64)
                    .map(|value| format!("Page {value}")),
                extra,
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        vec![manga::text_page(
            "No readable pages found. Log in via WebView or purchase the chapter if required.",
        )]
    } else {
        pages
    }
}

fn first_viewer_id(value: &Value) -> Option<String> {
    value
        .pointer("/episode/content")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("viewer"))
                .then(|| text_value(item.get("viewerId")))
                .flatten()
        })
}

fn parse_description(value: Option<&Value>) -> Option<String> {
    let raw = text_value(value)?;
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|nodes| nodes.as_array().cloned())
        .map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|node| {
                    node.get("children")
                        .and_then(Value::as_array)
                        .map(|children| {
                            children
                                .iter()
                                .filter_map(|child| text_value(child.get("text")))
                                .collect::<String>()
                        })
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
        .or(Some(raw))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/series/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
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

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for part in path {
        current = if let Ok(index) = part.parse::<usize>() {
            current.as_array()?.get(index)?
        } else {
            current.get(*part)?
        };
    }
    text_value(Some(current))
}

fn json_value(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn sample_chapter() -> MangaChapter {
    MangaChapter {
        key: "/episodes/sample".into(),
        title: Some("Sample".into()),
        url: Some(format!("{BASE_URL}/episodes/sample")),
        ..Default::default()
    }
}

const LIST_FIXTURE: &str = r#"<div class="series-list-item"><a class="series-list-item-link" href="/series/sample"><img class="series-list-item-img" src="/cover.jpg"><div class="series-list-item-h"><span>Sample TakeComic</span></div></a></div>"#;
const SEARCH_FIXTURE: &str = r#"{"searchResult":{"series":{"total":1,"series":[{"id":"sample","name":"Sample TakeComic","images":[{"url":"https://takecomic.jp/cover.jpg"}]}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"summary":{"name":"Sample TakeComic","description":"Sample description.","author":[{"name":"Sample Author"}],"images":[{"url":"https://takecomic.jp/cover.jpg"}],"tag":[{"name":"Sample"}],"isCompleted":false},"episodes":[]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"series":{"episodes":[{"id":"sample","title":"Sample chapter","datePublished":1700000000}]}}"#;
const ACCESS_FIXTURE: &str = r#"{"seriesAccess":{"episodeAccesses":[{"episodeId":"sample","hasAccess":true,"isCampaign":false}]}}"#;
const EPISODE_FIXTURE: &str =
    r#"{"episode":{"content":[{"type":"viewer","viewerId":"sample-viewer"}]}}"#;
const USER_FIXTURE: &str = r#"{"user":null}"#;
const PAGES_FIXTURE: &str = r#"{"totalPages":1,"result":[{"imageUrl":"https://takecomic.jp/page1.jpg","scramble":"","sort":1}]}"#;

export_manga_source!(SOURCE);
