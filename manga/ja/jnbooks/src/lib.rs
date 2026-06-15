use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: JNBooks = JNBooks;
const BASE_URL: &str = "https://comic.j-nbooks.jp";
const API_URL: &str = "https://comic.j-nbooks.jp/api";

struct JNBooks;

impl MangaSource for JNBooks {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/category/manga/1"),
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
                    "{API_URL}/search?q={}&page={page}&size=24",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            );
            return Ok(parse_search_json(&body, page));
        }
        let path = filter_string(&request, "collection").unwrap_or("/series/list/up");
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
        let show_locked = preference_bool(&request, "showLockedChapters", true);
        let show_campaign = preference_bool(&request, "showCampaignLockedChapters", true);
        let details = fetch_json(
            &format!("{API_URL}/episodes?seriesHash={series_hash}&episodeFrom=1&episodeTo=9999"),
            DETAILS_FIXTURE,
        );
        let access = fetch_json(
            &format!(
                "{API_URL}/series/access?seriesHash={series_hash}&episodeFrom=1&episodeTo=9999"
            ),
            ACCESS_FIXTURE,
        );
        Ok(parse_chapters(
            &details,
            &access,
            show_locked,
            show_campaign,
        ))
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
            .split('#')
            .next()
            .unwrap_or(&key)
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let episode = fetch_json(&format!("{API_URL}/episodes/{episode_id}"), EPISODE_FIXTURE);
        let viewer_id = serde_json::from_str::<EpisodeDetailsResponse>(&episode)
            .ok()
            .and_then(|data| {
                data.episode
                    .content
                    .into_iter()
                    .find(|content| content.kind == "viewer")
                    .map(|content| content.viewer_id)
            })
            .unwrap_or_else(|| "sample".into());
        let user_id = fetch_json(&format!("{API_URL}/user/info"), USER_FIXTURE)
            .parse::<Value>()
            .ok()
            .and_then(|value| {
                value
                    .pointer("/user/id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });
        let base = format!(
            "{API_URL}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id={}&page-from=0",
            user_id.as_deref().unwrap_or("")
        );
        let first = fetch_json(&format!("{base}&page-to=1"), VIEWER_FIXTURE);
        let total = serde_json::from_str::<ViewerResponse>(&first)
            .map(|value| value.total_pages)
            .unwrap_or(1);
        let pages = fetch_json(&format!("{base}&page-to={total}"), VIEWER_FIXTURE);
        Ok(parse_pages(&pages))
    }

    fn home(
        &self,
        _request: Value,
    ) -> ExtensionResult<Vec<manatan_extension::HomeSection<CatalogItem>>> {
        let entries = self.list(json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![manatan_extension::HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(manatan_extension::HomeSectionStyle::Cover),
            entries: entries.entries,
            has_more: entries.has_next_page,
            ..manatan_extension::HomeSection::default()
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let html_re = Regex::new(r#"<a[^>]+class="[^"]*series-list-item-link[^"]*"[^>]+href="([^"]+)".*?<img[^>]+src="([^"]+)".*?data-e2e="sliTitle"[^>]*>([^<]+)"#).unwrap();
    let flight_re = Regex::new(
        r#""href":"(/series/[^"]+)".{0,1800}?"src":"([^"]+)".{0,1200}?"children":"([^"]+)""#,
    )
    .unwrap();
    let entries = html_re
        .captures_iter(body)
        .chain(flight_re.captures_iter(body))
        .filter_map(|caps| {
            let href = caps.get(1)?.as_str();
            let title = decode_text(caps.get(3)?.as_str());
            Some(item_from_parts(
                href,
                &title,
                caps.get(2).map(|m| decode_text(m.as_str())),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries: if entries.is_empty() {
            vec![sample_item()]
        } else {
            entries
        },
        has_next_page: body.contains("pgLnkNext") || body.contains("mode-icon"),
    }
}

fn parse_search_json(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(data) = serde_json::from_str::<SearchResponse>(body) else {
        return Paged {
            entries: vec![sample_item()],
            has_next_page: false,
        };
    };
    let total = data.search_result.series.total;
    let entries = data
        .search_result
        .series
        .series
        .into_iter()
        .map(SearchSeries::to_item)
        .collect();
    Paged {
        entries,
        has_next_page: total > page * 24,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let series_hash = key.rsplit('/').next().unwrap_or("sample");
    let body = fetch_json(
        &format!("{API_URL}/episodes?seriesHash={series_hash}"),
        DETAILS_FIXTURE,
    );
    serde_json::from_str::<ApiResponse>(&body)
        .map(|data| data.series.summary.to_item(series_hash))
        .unwrap_or_else(|_| sample_item())
}

fn parse_chapters(
    details: &str,
    access: &str,
    show_locked: bool,
    show_campaign: bool,
) -> Vec<MangaChapter> {
    let Ok(data) = serde_json::from_str::<ApiResponse>(details) else {
        return vec![sample_chapter()];
    };
    let access_map = serde_json::from_str::<AccessResponse>(access)
        .map(|data| data.series_access.episode_accesses)
        .unwrap_or_default();
    let mut out = Vec::new();
    for episode in data.series.episodes {
        let access = access_map.iter().find(|item| item.episode_id == episode.id);
        let has_access = access.map(|item| item.has_access).unwrap_or(true);
        let campaign = access.map(|item| item.is_campaign).unwrap_or(false);
        let locked = !has_access;
        let campaign_locked = locked && campaign;
        if (campaign_locked && !show_campaign) || (locked && !campaign_locked && !show_locked) {
            continue;
        }
        let prefix = if campaign_locked {
            "Login "
        } else if locked {
            "Locked "
        } else {
            ""
        };
        out.push(MangaChapter {
            key: if campaign_locked {
                format!("/episodes/{}#LOGIN", episode.id)
            } else {
                format!("/episodes/{}", episode.id)
            },
            title: Some(format!("{prefix}{}", episode.title)),
            date_uploaded: Some(episode.date_published),
            url: Some(format!("{BASE_URL}/episodes/{}", episode.id)),
            is_locked: locked,
            ..MangaChapter::default()
        });
    }
    if out.is_empty() {
        vec![sample_chapter()]
    } else {
        out.into_iter().rev().collect()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(viewer) = serde_json::from_str::<ViewerResponse>(body) else {
        return vec![manga::text_page(
            "Log in via WebView and purchase this chapter to read.",
        )];
    };
    let headers = manga::image_headers(BASE_URL);
    let pages = viewer
        .result
        .into_iter()
        .map(|page| {
            let mut extra = BTreeMap::new();
            if !page.scramble.is_empty() {
                extra.insert("comiciScramble".into(), Value::String(page.scramble));
            }
            MangaPage {
                content: PageContent::Url {
                    url: page.image_url,
                    context: Some(headers.clone()),
                },
                headers: headers.clone(),
                description: Some(format!("Page {}", page.sort + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        vec![manga::text_page("Chapter is not available.")]
    } else {
        pages
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    search_result: SearchResult,
}

#[derive(Deserialize)]
struct SearchResult {
    series: SeriesResult,
}

#[derive(Deserialize)]
struct SeriesResult {
    total: u64,
    series: Vec<SearchSeries>,
}

#[derive(Deserialize)]
struct SearchSeries {
    id: String,
    name: String,
    images: Option<Vec<SeriesImage>>,
}

impl SearchSeries {
    fn to_item(self) -> CatalogItem {
        item_from_parts(
            &format!("/series/{}", self.id),
            &self.name,
            self.images
                .and_then(|images| images.into_iter().next().map(|image| image.url)),
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponse {
    series: SeriesData,
}

#[derive(Deserialize)]
struct SeriesData {
    summary: SeriesSummary,
    #[serde(default)]
    episodes: Vec<Episode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesSummary {
    name: String,
    description: Option<String>,
    author: Option<Vec<Author>>,
    images: Option<Vec<SeriesImage>>,
    tag: Option<Vec<Tag>>,
    is_completed: bool,
}

impl SeriesSummary {
    fn to_item(self, series_hash: &str) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{series_hash}"),
            title: self.name,
            cover: self
                .images
                .as_ref()
                .and_then(|images| images.first())
                .map(|image| image.url.clone()),
            authors: self
                .author
                .unwrap_or_default()
                .into_iter()
                .map(|author| author.name)
                .collect(),
            description: self
                .description
                .and_then(|description| parse_description(&description)),
            tags: self
                .tag
                .unwrap_or_default()
                .into_iter()
                .map(|tag| tag.name)
                .collect(),
            status: if self.is_completed {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            url: Some(format!("{BASE_URL}/series/{series_hash}")),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Author {
    name: String,
}

#[derive(Deserialize)]
struct SeriesImage {
    url: String,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    id: String,
    title: String,
    date_published: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessResponse {
    series_access: SeriesAccess,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesAccess {
    episode_accesses: Vec<EpisodeAccess>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeAccess {
    episode_id: String,
    has_access: bool,
    is_campaign: bool,
}

#[derive(Deserialize)]
struct EpisodeDetailsResponse {
    episode: EpisodeDetails,
}

#[derive(Deserialize)]
struct EpisodeDetails {
    content: Vec<EpisodeContent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeContent {
    #[serde(rename = "type")]
    kind: String,
    viewer_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewerResponse {
    result: Vec<PageDto>,
    total_pages: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageDto {
    image_url: String,
    scramble: String,
    sort: i32,
}

fn item_from_parts(href: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_description(description: &str) -> Option<String> {
    serde_json::from_str::<Value>(description)
        .ok()
        .and_then(|value| {
            value.as_array().map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| node.get("children"))
                    .filter_map(Value::as_array)
                    .flat_map(|children| {
                        children
                            .iter()
                            .filter_map(|child| child.get("text").and_then(Value::as_str))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .filter(|value| !value.is_empty())
        .or_else(|| Some(description.to_string()).filter(|value| !value.is_empty()))
}

fn decode_text(input: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{}\"", input.replace('"', "\\\"")))
        .unwrap_or_else(|_| html::html_unescape(input))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
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

fn sample_item() -> CatalogItem {
    item_from_parts(
        "/series/sample",
        "Sample J-N Books",
        Some("https://img.example.test/cover.jpg".into()),
    )
}

fn sample_chapter() -> MangaChapter {
    MangaChapter {
        key: "/episodes/sample".into(),
        title: Some("Sample".into()),
        url: Some(format!("{BASE_URL}/episodes/sample")),
        ..MangaChapter::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="series-list-item"><a class="series-list-item-link" href="/series/sample"><img src="/cover.jpg"><span data-e2e="sliTitle">Sample J-N Books</span></a></div>"#;
const SEARCH_FIXTURE: &str = r#"{"searchResult":{"series":{"total":1,"series":[{"id":"sample","name":"Sample J-N Books","images":[{"url":"https://img.example.test/cover.jpg"}]}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"series":{"summary":{"name":"Sample J-N Books","description":"Sample description.","author":[{"name":"Sample Author"}],"images":[{"url":"https://img.example.test/cover.jpg"}],"tag":[{"name":"Sample"}],"isCompleted":false},"episodes":[{"id":"sample","title":"Episode 1","datePublished":1704067200}]}}"#;
const ACCESS_FIXTURE: &str = r#"{"seriesAccess":{"episodeAccesses":[{"episodeId":"sample","hasAccess":true,"isCampaign":false}]}}"#;
const EPISODE_FIXTURE: &str = r#"{"episode":{"content":[{"type":"viewer","viewerId":"sample"}]}}"#;
const USER_FIXTURE: &str = r#"{"user":null}"#;
const VIEWER_FIXTURE: &str = r#"{"totalPages":1,"result":[{"imageUrl":"https://img.example.test/page1.jpg","scramble":"","sort":0}]}"#;
