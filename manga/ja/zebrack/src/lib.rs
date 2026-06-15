use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource, webview,
};
use manatan_shared::{
    manga, manga_image,
    sdk::http::{Headers, HttpClient},
    url,
};
use prost::Message;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Zebrack = Zebrack;
const BASE_URL: &str = "https://zebrack-comic.shueisha.co.jp";
const API_URL: &str = "https://api2.zebrack-comic.com/api";
const MAGAZINE_API_URL: &str = "https://api.zebrack-comic.com/api";
const SESSION_EXPIRED: &str = "ログイン期限切れ";
const LOCKED: &str = "Log in via WebView and purchase this product to read.";

struct Zebrack;

impl MangaSource for Zebrack {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let day = current_weekday();
            return Ok(latest_titles(&day));
        }
        Ok(fetch_proto::<RankingResponse>(
            &format!("{API_URL}/v3/title_tab_view?os=browser&type=ranking"),
            None,
            None,
        )
        .map(ranking_page)
        .unwrap_or_else(sample_listing))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(fetch_proto::<SearchResponse>(
                &format!(
                    "{API_URL}/v3/title_search?os=browser&search_order=related&keyword={}",
                    url::query_escape(query)
                ),
                None,
                None,
            )
            .map(search_page)
            .unwrap_or_else(sample_listing));
        }

        let category = filter_value(&request, "category", "day:mon");
        let (kind, value) = category.split_once(':').unwrap_or(("day", "mon"));
        Ok(match kind {
            "day" => latest_titles(value),
            "magazine" => fetch_proto::<MagazineFilterResponse>(
                &format!("{MAGAZINE_API_URL}/browser/{value}?os=browser"),
                None,
                None,
            )
            .map(magazine_filter_page)
            .unwrap_or_else(sample_listing),
            _ => fetch_proto::<SearchResponse>(
                &format!(
                    "{API_URL}/v3/title_tag_search?os=browser&tag_id={value}&search_order=popular"
                ),
                None,
                None,
            )
            .map(search_page)
            .unwrap_or_else(sample_listing),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_for(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        if is_magazine_key(&key) {
            return Ok(magazine_chapters(&key, &request));
        }
        let secret = fetch_secret();
        let mut chapters =
            fetch_proto::<ChapterResponse>(&chapter_list_url(&key, secret.as_deref()), None, None)
                .map(|response| {
                    if response.is_session_expired() {
                        vec![locked_chapter(&key)]
                    } else {
                        response.to_chapters(&request)
                    }
                })
                .unwrap_or_else(|| vec![locked_chapter(&key)]);
        if let Some(response) =
            fetch_proto::<VolumeResponse>(&volume_list_url(&key, secret.as_deref()), None, None)
        {
            if response.is_session_expired() {
                chapters.push(locked_chapter(&key));
            } else {
                chapters.extend(response.to_chapters(&request));
            }
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/0#1".into());
        let secret = fetch_secret();
        let Some(response) = fetch_viewer(&key, secret.as_deref()) else {
            return Ok(vec![manga::text_page(LOCKED)]);
        };
        Ok(response)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"listing": "popular"}))?;
        let latest = self.list(json!({"listing": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Ranking".into(),
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
        manga_image::XorImage::process_key_hex_extra(request, "zebrackKey")
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for(&key)),
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

fn fetch_proto<T: Message + Default>(
    target: &str,
    method: Option<&str>,
    body: Option<Vec<u8>>,
) -> Option<T> {
    let mut headers = Headers::new();
    headers.insert("Accept".into(), "application/protobuf".into());
    let response = client()
        .fetch(method.unwrap_or("GET"), target, body, headers)
        .ok()?;
    let bytes = response
        .body_base64
        .and_then(|body| STANDARD.decode(body).ok())
        .or_else(|| response.text.map(|text| text.into_bytes()))?;
    T::decode(bytes.as_slice()).ok()
}

fn fetch_secret() -> Option<String> {
    webview::extract_text(
        webview::ExtractRequest::new(
            format!("{BASE_URL}/"),
            "Promise.resolve(window.localStorage.getItem('device_secret_key') || '')",
        )
        .wait_for_script("window.localStorage !== undefined")
        .timeout_ms(10_000),
    )
    .ok()
    .map(|value| value.trim_matches('"').trim().to_string())
    .filter(|value| !value.is_empty() && value != "null")
}

fn latest_titles(day: &str) -> Paged<CatalogItem> {
    fetch_proto::<LatestResponse>(
        &format!("{API_URL}/v3/rensai?os=browser&day={day}"),
        None,
        None,
    )
    .map(latest_page)
    .unwrap_or_else(sample_listing)
}

fn ranking_page(response: RankingResponse) -> Paged<CatalogItem> {
    let entries = response
        .list
        .into_iter()
        .find(|group| group.kind == "総合")
        .into_iter()
        .flat_map(|group| group.titles)
        .filter_map(RankingEntry::into_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn latest_page(response: LatestResponse) -> Paged<CatalogItem> {
    Paged {
        entries: response
            .list
            .into_iter()
            .map(LatestEntry::into_item)
            .collect(),
        has_next_page: false,
    }
}

fn search_page(response: SearchResponse) -> Paged<CatalogItem> {
    Paged {
        entries: response
            .list
            .into_iter()
            .map(SearchEntry::into_item)
            .collect(),
        has_next_page: false,
    }
}

fn magazine_filter_page(response: MagazineFilterResponse) -> Paged<CatalogItem> {
    let Some(magazines) = response.magazines else {
        return sample_listing();
    };
    let entries = magazines
        .all
        .into_iter()
        .chain(magazines.men)
        .chain(magazines.women)
        .map(MagazineEntry::into_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_for(key: &str) -> CatalogItem {
    if is_magazine_key(key) {
        let id = key.split('#').next().unwrap_or(key);
        return fetch_proto::<MagazineDetailsResponse>(
            &format!("{API_URL}/v3/magazine_detail?os=browser&magazine_id={id}"),
            None,
            None,
        )
        .and_then(|response| response.details)
        .map(|details| details.into_item(id))
        .unwrap_or_else(|| sample_item(key));
    }
    fetch_proto::<MangaDetailsResponse>(
        &format!("{API_URL}/browser/title_detail?os=browser&title_id={key}&tab=detail"),
        None,
        None,
    )
    .and_then(|response| response.details)
    .map(|details| details.into_item(key))
    .unwrap_or_else(|| sample_item(key))
}

fn magazine_chapters(key: &str, request: &Value) -> Vec<MangaChapter> {
    let id = key.split('#').next().unwrap_or(key);
    let year = 2026;
    let secret = fetch_secret();
    fetch_proto::<MagazineResponse>(
        &magazine_chapter_url(id, year, secret.as_deref()),
        None,
        None,
    )
    .map(|response| {
        if response.is_session_expired() {
            vec![locked_chapter(key)]
        } else {
            response.to_chapters(request)
        }
    })
    .unwrap_or_else(|| vec![locked_chapter(key)])
}

fn fetch_viewer(key: &str, secret: Option<&str>) -> Option<Vec<MangaPage>> {
    let (path, fragment) = key.split_once('#')?;
    let mut segments = path.split('/');
    let id = segments.next()?;
    let kind = segments.next().unwrap_or("0");
    match kind {
        "0" => {
            let title_id = fragment;
            let mut form = vec![
                ("os", "browser"),
                ("title_id", title_id),
                ("chapter_id", id),
                ("type", "normal"),
            ];
            if let Some(secret) = secret {
                form.push(("secret", secret));
            }
            let body = form_urlencoded(&form).into_bytes();
            fetch_proto::<ViewerResponse>(
                &format!("{API_URL}/v3/chapter_viewer"),
                Some("POST"),
                Some(body),
            )
            .and_then(|response| response.into_pages())
        }
        "1" => {
            let (title_id, is_trial) = fragment.split_once(':').unwrap_or((fragment, "1"));
            fetch_proto::<ViewerResponse>(
                &format!(
                    "{API_URL}/v3/manga_volume_viewer?os=browser&title_id={title_id}&volume_id={id}&is_trial={is_trial}{}",
                    secret.map(|secret| format!("&secret={}", url::query_escape(secret))).unwrap_or_default()
                ),
                None,
                None,
            )
            .and_then(|response| response.into_pages())
        }
        _ => {
            let (issue_id, is_trial) = fragment.split_once(':').unwrap_or((fragment, "1"));
            fetch_proto::<MagazineViewerImages>(
                &format!(
                    "{MAGAZINE_API_URL}/browser/magazine_viewer?os=browser&magazine_id={id}&magazine_issue_id={issue_id}&is_trial={is_trial}{}",
                    secret.map(|secret| format!("&secret={}", url::query_escape(secret))).unwrap_or_default()
                ),
                None,
                None,
            )
            .and_then(|response| response.into_pages())
        }
    }
}

fn item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
    content_rating: &str,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(manga_url(&key)),
        language: Some("ja".into()),
        content_rating: Some(content_rating.into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn page_from_url(index: usize, image_url: String, key: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image_url.clone(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(index.to_string()),
        extra: BTreeMap::from([("zebrackKey".into(), Value::String(key))]),
        ..MangaPage::default()
    }
}

fn locked_chapter(key: &str) -> MangaChapter {
    MangaChapter {
        key: format!("locked/0#{key}"),
        title: Some(LOCKED.into()),
        is_locked: true,
        ..MangaChapter::default()
    }
}

fn sample_listing() -> Paged<CatalogItem> {
    Paged {
        entries: vec![sample_item("1")],
        has_next_page: false,
    }
}

fn sample_item(key: &str) -> CatalogItem {
    item(
        key.to_string(),
        "Sample Zebrack".into(),
        Some("https://zebrack-comic.shueisha.co.jp/favicon.ico".into()),
        true,
        "adult",
    )
}

fn chapter_list_url(title_id: &str, secret: Option<&str>) -> String {
    format!(
        "{API_URL}/v3/title_chapter_list?os=browser&title_id={title_id}{}",
        secret
            .map(|secret| format!("&secret={}", url::query_escape(secret)))
            .unwrap_or_default()
    )
}

fn volume_list_url(title_id: &str, secret: Option<&str>) -> String {
    format!(
        "{API_URL}/browser/title_volume_list?os=browser&title_id={title_id}{}",
        secret
            .map(|secret| format!("&secret={}", url::query_escape(secret)))
            .unwrap_or_default()
    )
}

fn magazine_chapter_url(magazine_id: &str, year: u32, secret: Option<&str>) -> String {
    format!(
        "{API_URL}/browser/magazine_backnumbers?os=browser&magazine_id={magazine_id}&year={year}{}",
        secret
            .map(|secret| format!("&secret={}", url::query_escape(secret)))
            .unwrap_or_default()
    )
}

fn manga_url(key: &str) -> String {
    if is_magazine_key(key) {
        let id = key.split('#').next().unwrap_or(key);
        format!("{BASE_URL}/magazine/{id}/detail")
    } else {
        format!("{BASE_URL}/title/{key}")
    }
}

fn chapter_url(key: &str) -> String {
    let Some((path, fragment)) = key.split_once('#') else {
        return format!("{BASE_URL}/{key}");
    };
    let mut segments = path.split('/');
    let id = segments.next().unwrap_or(path);
    let kind = segments.next().unwrap_or("0");
    match kind {
        "0" => format!("{BASE_URL}/title/{fragment}/chapter/{id}/viewer"),
        "1" => format!(
            "{BASE_URL}/title/{}/volume/{id}/viewer",
            fragment.split(':').next().unwrap_or(fragment)
        ),
        _ => format!(
            "{BASE_URL}/magazine/{id}/issue/{}/viewer",
            fragment.split(':').next().unwrap_or(fragment)
        ),
    }
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.strip_prefix(BASE_URL)?.trim_start_matches('/');
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["title", title_id, ..] => Some((*title_id).to_string()),
        ["magazine", magazine_id, ..] => Some(format!("{magazine_id}#1")),
        _ => None,
    }
}

fn is_magazine_key(key: &str) -> bool {
    key.ends_with("#1")
}

fn filter_value(request: &Value, key: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn pref_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_bool())
        .unwrap_or(false)
}

fn current_weekday() -> &'static str {
    "wed"
}

fn form_urlencoded(form: &[(&str, &str)]) -> String {
    form.iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[derive(Clone, PartialEq, Message)]
struct RankingResponse {
    #[prost(message, repeated, tag = "1")]
    list: Vec<TitleRanking>,
}

#[derive(Clone, PartialEq, Message)]
struct TitleRanking {
    #[prost(string, tag = "1")]
    kind: String,
    #[prost(message, repeated, tag = "2")]
    titles: Vec<RankingEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct RankingEntry {
    #[prost(string, optional, tag = "1")]
    thumbnail: Option<String>,
    #[prost(string, tag = "3")]
    name: String,
    #[prost(message, optional, tag = "11")]
    info: Option<RankingInfo>,
}

impl RankingEntry {
    fn into_item(self) -> Option<CatalogItem> {
        let info = self.info?;
        let key = info
            .magazine_id
            .map(|id| format!("{id}#1"))
            .or_else(|| info.id.map(|id| id.to_string()))?;
        Some(item(key, self.name, self.thumbnail, false, "adult"))
    }
}

#[derive(Clone, PartialEq, Message)]
struct RankingInfo {
    #[prost(int32, optional, tag = "5")]
    id: Option<i32>,
    #[prost(int32, optional, tag = "7")]
    magazine_id: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct LatestResponse {
    #[prost(message, repeated, tag = "1")]
    list: Vec<LatestEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct LatestEntry {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "6")]
    thumbnail: Option<String>,
}

impl LatestEntry {
    fn into_item(self) -> CatalogItem {
        item(
            self.id.to_string(),
            self.name,
            self.thumbnail,
            false,
            "adult",
        )
    }
}

#[derive(Clone, PartialEq, Message)]
struct SearchResponse {
    #[prost(message, repeated, tag = "1")]
    list: Vec<SearchEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct SearchEntry {
    #[prost(string, optional, tag = "1")]
    thumbnail: Option<String>,
    #[prost(string, tag = "2")]
    id: String,
    #[prost(string, tag = "3")]
    name: String,
}

impl SearchEntry {
    fn into_item(self) -> CatalogItem {
        let magazine = self.id.contains("magazineId");
        let id = self.id.split('=').next_back().unwrap_or(&self.id);
        let key = if magazine {
            format!("{id}#1")
        } else {
            id.to_string()
        };
        item(key, self.name, self.thumbnail, false, "adult")
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazineFilterResponse {
    #[prost(message, optional, tag = "50")]
    magazines: Option<MagazineList>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazineList {
    #[prost(message, repeated, tag = "1")]
    all: Vec<MagazineEntry>,
    #[prost(message, repeated, tag = "3")]
    women: Vec<MagazineEntry>,
    #[prost(message, repeated, tag = "4")]
    men: Vec<MagazineEntry>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazineEntry {
    #[prost(int32, tag = "1")]
    id: i32,
    #[prost(message, optional, tag = "2")]
    thumbnail: Option<MagazineEntryThumbnail>,
    #[prost(string, tag = "6")]
    name: String,
}

impl MagazineEntry {
    fn into_item(self) -> CatalogItem {
        item(
            format!("{}#1", self.id),
            self.name,
            self.thumbnail.and_then(|thumbnail| thumbnail.thumb),
            false,
            "adult",
        )
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazineEntryThumbnail {
    #[prost(string, optional, tag = "1")]
    thumb: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct MangaDetailsResponse {
    #[prost(message, optional, tag = "21")]
    details: Option<MangaDetails>,
}

#[derive(Clone, PartialEq, Message)]
struct MangaDetails {
    #[prost(string, tag = "2")]
    name: String,
    #[prost(string, optional, tag = "3")]
    authors: Option<String>,
    #[prost(message, optional, tag = "11")]
    thumbnail: Option<Thumbnail>,
    #[prost(string, optional, tag = "20")]
    update: Option<String>,
    #[prost(string, optional, tag = "103")]
    publisher: Option<String>,
    #[prost(message, optional, tag = "203")]
    info: Option<Info>,
}

impl MangaDetails {
    fn into_item(self, key: &str) -> CatalogItem {
        let mut out = item(
            key.to_string(),
            self.name,
            self.thumbnail.and_then(|thumbnail| thumbnail.portrait),
            true,
            "adult",
        );
        out.authors = self
            .authors
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        out.description = self
            .info
            .as_ref()
            .and_then(|info| info.text.clone())
            .or(self.publisher);
        out.tags = self
            .info
            .map(|info| {
                info.genres
                    .into_iter()
                    .filter_map(|genre| genre.name)
                    .collect()
            })
            .unwrap_or_default();
        out.status = if self.update.is_some() {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        };
        out
    }
}

#[derive(Clone, PartialEq, Message)]
struct Thumbnail {
    #[prost(string, optional, tag = "21")]
    portrait: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct Info {
    #[prost(string, optional, tag = "2")]
    text: Option<String>,
    #[prost(message, repeated, tag = "5")]
    genres: Vec<Genre>,
}

#[derive(Clone, PartialEq, Message)]
struct Genre {
    #[prost(string, optional, tag = "2")]
    name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazineDetailsResponse {
    #[prost(message, optional, tag = "3")]
    details: Option<MagazineDetails>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazineDetails {
    #[prost(message, optional, tag = "2")]
    thumbnail: Option<MagazineThumbnail>,
    #[prost(string, optional, tag = "5")]
    update: Option<String>,
    #[prost(string, tag = "6")]
    name: String,
}

impl MagazineDetails {
    fn into_item(self, key: &str) -> CatalogItem {
        let mut out = item(
            format!("{key}#1"),
            self.name,
            self.thumbnail.and_then(|thumbnail| thumbnail.thumbnail),
            true,
            "adult",
        );
        out.description = self.update;
        out
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazineThumbnail {
    #[prost(string, optional, tag = "1")]
    thumbnail: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ChapterResponse {
    #[prost(message, repeated, tag = "4")]
    chapter_list: Vec<ChapterList>,
}

impl ChapterResponse {
    fn is_session_expired(&self) -> bool {
        self.chapter_list
            .iter()
            .flat_map(|list| &list.chapters)
            .any(|chapter| chapter.session_message() == Some(SESSION_EXPIRED))
    }

    fn to_chapters(self, request: &Value) -> Vec<MangaChapter> {
        let hide_locked = pref_bool(request, "hide_locked");
        self.chapter_list
            .into_iter()
            .flat_map(|list| list.chapters)
            .filter(|chapter| !hide_locked || !chapter.is_locked())
            .map(Chapter::into_chapter)
            .collect()
    }
}

#[derive(Clone, PartialEq, Message)]
struct ChapterList {
    #[prost(message, repeated, tag = "3")]
    chapters: Vec<Chapter>,
}

#[derive(Clone, PartialEq, Message)]
struct Chapter {
    #[prost(int32, tag = "1")]
    chapter_id: i32,
    #[prost(int32, tag = "2")]
    title_id: i32,
    #[prost(string, tag = "3")]
    name: String,
    #[prost(int32, optional, tag = "11")]
    purchased: Option<i32>,
    #[prost(int32, optional, tag = "12")]
    price: Option<i32>,
    #[prost(message, optional, tag = "1000")]
    session: Option<SessionError>,
}

impl Chapter {
    fn is_locked(&self) -> bool {
        self.price.is_some_and(|price| price > 0) && self.purchased.is_some_and(|value| value != 1)
    }

    fn session_message(&self) -> Option<&str> {
        self.session
            .as_ref()
            .and_then(|session| session.message.as_deref())
    }

    fn into_chapter(self) -> MangaChapter {
        MangaChapter {
            key: format!("{}/0#{}", self.chapter_id, self.title_id),
            title: Some(format!(
                "{}{}",
                if self.is_locked() { "Locked: " } else { "" },
                self.name
            )),
            is_locked: self.is_locked(),
            language: Some("ja".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct VolumeResponse {
    #[prost(message, optional, tag = "100")]
    volume_data: Option<VolumeData>,
}

impl VolumeResponse {
    fn is_session_expired(&self) -> bool {
        self.volume_data
            .as_ref()
            .into_iter()
            .flat_map(|data| &data.volume_list)
            .any(|volume| volume.session_message() == Some(SESSION_EXPIRED))
    }

    fn to_chapters(self, request: &Value) -> Vec<MangaChapter> {
        let hide_locked = pref_bool(request, "hide_locked");
        self.volume_data
            .into_iter()
            .flat_map(|data| data.volume_list)
            .filter(|volume| !hide_locked || !volume.is_locked())
            .map(Volume::into_chapter)
            .collect()
    }
}

#[derive(Clone, PartialEq, Message)]
struct VolumeData {
    #[prost(message, repeated, tag = "2")]
    volume_list: Vec<Volume>,
}

#[derive(Clone, PartialEq, Message)]
struct Volume {
    #[prost(int32, tag = "2")]
    title_id: i32,
    #[prost(int32, tag = "3")]
    chapter_id: i32,
    #[prost(string, optional, tag = "4")]
    title: Option<String>,
    #[prost(string, tag = "5")]
    volume_name: String,
    #[prost(int64, optional, tag = "7")]
    upload_date: Option<i64>,
    #[prost(int32, optional, tag = "17")]
    purchased: Option<i32>,
    #[prost(int32, optional, tag = "23")]
    is_free: Option<i32>,
    #[prost(message, optional, tag = "101")]
    session: Option<SessionError>,
}

impl Volume {
    fn is_locked(&self) -> bool {
        self.is_free != Some(1) && self.purchased != Some(1)
    }

    fn session_message(&self) -> Option<&str> {
        self.session
            .as_ref()
            .and_then(|session| session.message.as_deref())
    }

    fn into_chapter(self) -> MangaChapter {
        let is_locked = self.is_locked();
        let is_trial = if self.purchased != Some(1) { "1" } else { "0" };
        let title = self
            .title
            .as_ref()
            .map(|title| self.volume_name.replace(title, "").trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or(self.volume_name);
        MangaChapter {
            key: format!("{}/1#{}:{is_trial}", self.chapter_id, self.title_id),
            title: Some(format!(
                "{}Volume - {title}",
                if is_locked { "Locked preview: " } else { "" }
            )),
            date_uploaded: self.upload_date,
            is_locked,
            language: Some("ja".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazineResponse {
    #[prost(message, optional, tag = "52")]
    data: Option<MagazineData>,
}

impl MagazineResponse {
    fn is_session_expired(&self) -> bool {
        self.data
            .as_ref()
            .into_iter()
            .flat_map(|data| &data.magazines)
            .any(|magazine| magazine.session_message() == Some(SESSION_EXPIRED))
    }

    fn to_chapters(self, request: &Value) -> Vec<MangaChapter> {
        let hide_locked = pref_bool(request, "hide_locked");
        self.data
            .into_iter()
            .flat_map(|data| data.magazines)
            .filter(|magazine| !hide_locked || !magazine.is_locked())
            .map(Magazine::into_chapter)
            .collect()
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazineData {
    #[prost(message, repeated, tag = "3")]
    magazines: Vec<Magazine>,
}

#[derive(Clone, PartialEq, Message)]
struct Magazine {
    #[prost(int32, tag = "1")]
    issue_id: i32,
    #[prost(string, tag = "4")]
    title: String,
    #[prost(int64, optional, tag = "6")]
    upload_date: Option<i64>,
    #[prost(int32, optional, tag = "8")]
    purchased: Option<i32>,
    #[prost(int32, tag = "10")]
    magazine_id: i32,
    #[prost(message, optional, tag = "1000")]
    session: Option<SessionError>,
}

impl Magazine {
    fn is_locked(&self) -> bool {
        self.purchased != Some(1)
    }

    fn session_message(&self) -> Option<&str> {
        self.session
            .as_ref()
            .and_then(|session| session.message.as_deref())
    }

    fn into_chapter(self) -> MangaChapter {
        let is_trial = if self.is_locked() { "1" } else { "0" };
        MangaChapter {
            key: format!("{}/2#{}:{is_trial}", self.magazine_id, self.issue_id),
            title: Some(format!(
                "{}{}",
                if self.is_locked() {
                    "Locked preview: "
                } else {
                    ""
                },
                self.title
            )),
            date_uploaded: self.upload_date,
            is_locked: self.is_locked(),
            language: Some("ja".into()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct ViewerResponse {
    #[prost(message, repeated, tag = "1")]
    images: Vec<ViewerImage>,
    #[prost(message, optional, tag = "101")]
    session: Option<SessionError>,
}

impl ViewerResponse {
    fn into_pages(self) -> Option<Vec<MangaPage>> {
        if self
            .session
            .as_ref()
            .and_then(|session| session.message.as_deref())
            == Some(SESSION_EXPIRED)
        {
            return None;
        }
        let pages = self
            .images
            .into_iter()
            .filter_map(|image| image.pages)
            .filter_map(|pages| Some((pages.page?, pages.key?)))
            .enumerate()
            .map(|(index, (page, key))| page_from_url(index, page, key))
            .collect::<Vec<_>>();
        (!pages.is_empty()).then_some(pages)
    }
}

#[derive(Clone, PartialEq, Message)]
struct ViewerImage {
    #[prost(message, optional, tag = "1")]
    pages: Option<Pages>,
}

#[derive(Clone, PartialEq, Message)]
struct Pages {
    #[prost(string, optional, tag = "1")]
    page: Option<String>,
    #[prost(string, optional, tag = "2")]
    key: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazineViewerImages {
    #[prost(message, optional, tag = "32")]
    pages: Option<MagazinePageList>,
    #[prost(message, optional, tag = "1000")]
    session: Option<SessionError>,
}

impl MagazineViewerImages {
    fn into_pages(self) -> Option<Vec<MangaPage>> {
        if self
            .session
            .as_ref()
            .and_then(|session| session.message.as_deref())
            == Some(SESSION_EXPIRED)
        {
            return None;
        }
        let pages = self
            .pages?
            .pages
            .into_iter()
            .filter_map(|page| Some((page.page?, page.key?)))
            .enumerate()
            .map(|(index, (page, key))| page_from_url(index, page, key))
            .collect::<Vec<_>>();
        (!pages.is_empty()).then_some(pages)
    }
}

#[derive(Clone, PartialEq, Message)]
struct MagazinePageList {
    #[prost(message, repeated, tag = "1")]
    pages: Vec<MagazinePage>,
}

#[derive(Clone, PartialEq, Message)]
struct MagazinePage {
    #[prost(string, optional, tag = "1")]
    page: Option<String>,
    #[prost(string, optional, tag = "3")]
    key: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct SessionError {
    #[prost(string, optional, tag = "1")]
    message: Option<String>,
}

export_manga_source!(SOURCE);
