use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Tapas = Tapas;
const BASE_URL: &str = "https://tapas.io";
const API_URL: &str = "https://story-api.tapas.io";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:105.0) Gecko/20100101 Firefox/105.0";

struct Tapas;

impl MangaSource for Tapas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_wrapper(POPULAR_FIXTURE));
        }
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .saturating_sub(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!(
                "{API_URL}/cosmos/api/v1/landing/genre?category_type=COMIC&sort_option=NEWEST_EPISODE&subtab_id=17&pageSize=25&page={page}"
            )
        } else {
            format!(
                "{API_URL}/cosmos/api/v1/landing/ranking?category_type=COMIC&subtab_id=17&size=25&page={page}"
            )
        };
        Ok(parse_manga_wrapper(&fetch_api(&target, POPULAR_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&format!("{BASE_URL}{key}/info"), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_search_html(&fetch_document(
            &format!(
                "{BASE_URL}/search?pageNumber={page}&q={}&t=COMICS",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1".to_string());
        Ok(parse_details(
            &fetch_document(&format!("{BASE_URL}{key}/info"), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1".to_string());
        let show_locked = preference_bool(&request, "lockedChapterVisibilityPref", true);
        let show_scheduled = preference_bool(&request, "scheduledChapterVisibilityPref", true);
        let mut page = 1;
        let mut chapters = Vec::new();
        loop {
            let target = format!(
                "{BASE_URL}{key}/episodes?page={page}&sort=NEWEST&since=0&large=true&last_access=0&="
            );
            let response = parse_chapters(
                &fetch_api(&target, CHAPTERS_FIXTURE),
                show_locked,
                show_scheduled,
            );
            chapters.extend(response.0);
            if !response.1 {
                break;
            }
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/1".to_string());
        let show_notes = preference_bool(&request, "showAuthorsNotes", true);
        Ok(parse_pages(
            &fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            show_notes,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}/info")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let input = request
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: input.to_string(),
                    ..SearchRequest::default()
                }),
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
        .with_header("User-Agent", USER_AGENT)
        .with_referer("https://m.tapas.io")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_wrapper(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<DataWrapper<WrapperContent>>(body)
        .unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).expect("popular fixture"));
    Paged {
        entries: response
            .data
            .items
            .into_iter()
            .map(MangaDto::to_item)
            .collect(),
        has_next_page: response
            .meta
            .is_some_and(|meta| !meta.pagination.last || meta.pagination.has_next),
    }
}

fn parse_search_html(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("search-item-wrap")
            .skip(1)
            .filter_map(|chunk| {
                let id = html::attr_after(chunk, "data-series-id", "data-series-id")
                    .or_else(|| html::attr_after(chunk, "<a", "data-series-id"))?;
                let title = html::attr_after(chunk, "<img", "alt")
                    .or_else(|| {
                        html::text_between(chunk, "title-section", "</a>")
                            .map(|v| html::strip_tags(&v))
                    })
                    .unwrap_or_else(|| "Tapas".to_string());
                Some(CatalogItem {
                    key: format!("/series/{id}"),
                    title,
                    cover: html::attr_after(chunk, "<img", "src"),
                    description: html::text_between(chunk, "desc force mbm", "</")
                        .map(|v| html::strip_tags(&v)),
                    url: Some(format!("{BASE_URL}/series/{id}")),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("paging__button--next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/1".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "info__right", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Tapas".to_string()),
        cover: html::attr_after(body, "js-thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src")),
        description: html::text_between(body, "description__body", "</")
            .map(|v| html::strip_tags(&v)),
        tags: body
            .split("genre-btn")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect(),
        authors: body
            .split("creator-section")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, "name", "</"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect(),
        status: if body.to_ascii_lowercase().contains("completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}{key}/info")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(
    body: &str,
    show_locked: bool,
    show_scheduled: bool,
) -> (Vec<MangaChapter>, bool) {
    let response = serde_json::from_str::<DataWrapper<ChapterListDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("chapters fixture"));
    let chapters = response
        .data
        .episodes
        .into_iter()
        .filter(|chapter| show_locked || chapter.unlocked || chapter.free)
        .filter(|chapter| show_scheduled || !chapter.scheduled)
        .map(ChapterDto::to_chapter)
        .collect();
    (chapters, response.data.pagination.has_next)
}

fn parse_pages(body: &str, show_notes: bool) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("content__img"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect::<Vec<_>>();
    if show_notes {
        if let Some(notes) = html::text_between(body, "js-episode-story", "</p>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
        {
            pages.push(manga::text_page(&notes));
        }
    }
    pages
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_end_matches("/info")
        .to_string()
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

#[derive(Debug, Deserialize)]
struct DataWrapper<T> {
    data: T,
    meta: Option<Meta>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    last: bool,
    #[serde(rename = "has_next")]
    has_next: bool,
}

#[derive(Debug, Deserialize)]
struct WrapperContent {
    items: Vec<MangaDto>,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    #[serde(rename = "seriesId")]
    series_id: i64,
    title: String,
    description: String,
    #[serde(rename = "assetProperty")]
    asset_property: AssetProperty,
}

impl MangaDto {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{}", self.series_id),
            title: self.title,
            cover: self.asset_property.thumbnail_url(),
            description: Some(self.description),
            url: Some(format!("{BASE_URL}/series/{}", self.series_id)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AssetProperty {
    #[serde(rename = "bookCoverImage")]
    book_cover_image: std::collections::HashMap<String, String>,
}

impl AssetProperty {
    fn thumbnail_url(self) -> Option<String> {
        self.book_cover_image
            .into_values()
            .next()
            .map(|value| format!("{value}.png"))
    }
}

#[derive(Debug, Deserialize)]
struct ChapterListDto {
    pagination: Pagination,
    episodes: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: i64,
    title: String,
    #[serde(rename = "publish_date")]
    date: String,
    unlocked: bool,
    free: bool,
    scene: f32,
    scheduled: bool,
}

impl ChapterDto {
    fn to_chapter(self) -> MangaChapter {
        let locked = !(self.unlocked || self.free);
        MangaChapter {
            key: format!("/episode/{}", self.id),
            title: Some(if locked {
                format!("Locked - {}", self.title)
            } else {
                self.title
            }),
            chapter_number: Some(self.scene),
            date_uploaded: self
                .date
                .split('T')
                .next()
                .and_then(manatan_shared::dates::parse_fixture_date),
            url: Some(format!("{BASE_URL}/episode/{}", self.id)),
            is_locked: locked,
            ..MangaChapter::default()
        }
    }
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"{"data":{"items":[{"seriesId":1,"title":"Sample Tapas","description":"Sample","assetProperty":{"bookCoverImage":{"large":"https://cdn.example.test/tapas-cover"}}}]},"meta":{"pagination":{"last":true,"has_next":false}}}"#;
const SEARCH_FIXTURE: &str = r#"<div class="search-item-wrap"><a data-series-id="1"><img alt="Sample Tapas" src="/cover.png"></a><div class="desc force mbm">Sample</div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="info__right"><h1 class="title">Sample Tapas</h1></div><div class="thumb js-thumbnail"><img src="/cover.png"></div><div class="description__body">Sample</div><a class="genre-btn">Fantasy</a><div class="creator-section"><span class="name">Creator</span></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"pagination":{"last":true,"has_next":false},"episodes":[{"id":1,"title":"Episode 1","publish_date":"2024-01-01T00:00:00Z","unlocked":true,"free":true,"scene":1.0,"scheduled":false}]},"meta":{"pagination":{"last":true,"has_next":false}}}"#;
const PAGES_FIXTURE: &str = r#"<img class="content__img" data-src="/page1.jpg"><p class="js-episode-story">Creator note</p>"#;
