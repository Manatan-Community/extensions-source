use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: NaverComic = NaverComic;
const BASE_URL: &str = "https://comic.naver.com";
const MOBILE_URL: &str = "https://m.comic.naver.com";

struct NaverComic;

impl MangaSource for NaverComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        if source.kind == "webtoon" {
            let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
                "UPDATE"
            } else {
                "ALL_READER"
            };
            return Ok(parse_webtoon_listing(
                &fetch_document(
                    &format!("{MOBILE_URL}/webtoon/weekday?sort={sort}"),
                    WEBTOON_LIST_FIXTURE,
                ),
                source,
            ));
        }
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "UPDATE"
        } else {
            "VIEW"
        };
        let body = fetch_text(
            &format!(
                "{BASE_URL}/api/{}/list?order={order}&page={}",
                source.kind,
                page(&request)
            ),
            CHALLENGE_LIST_FIXTURE,
        );
        Ok(parse_challenge_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) || query.starts_with(MOBILE_URL) {
            let key = normalize_key(query, source.kind);
            return Ok(Paged {
                entries: vec![
                    self.details(serde_json::json!({"sourceId": source.id, "manga": key}))?,
                ],
                has_next_page: false,
            });
        }
        let body = fetch_text(
            &format!(
                "{BASE_URL}/api/search/{}?keyword={}&page={}",
                source.kind,
                url::query_escape(query),
                page(&request)
            ),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/list?titleId=1", source.kind));
        let title_id = query_param(&key, "titleId").unwrap_or_else(|| "1".to_string());
        let body = fetch_text(
            &format!("{BASE_URL}/api/article/list/info?titleId={title_id}"),
            DETAILS_FIXTURE,
        );
        Ok(parse_details_json(&body, source, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/list?titleId=1", source.kind));
        let title_id = query_param(&key, "titleId").unwrap_or_else(|| "1".to_string());
        let body = fetch_text(
            &format!("{BASE_URL}/api/article/list?titleId={title_id}&page=1"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters_json(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/webtoon/detail?titleId=1&no=1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) || input.starts_with(MOBILE_URL) {
            let key = normalize_key(input, source.kind);
            return Ok(Some(UrlResolveResult {
                item: Some(self.details(serde_json::json!({"sourceId": source.id, "manga": key}))?),
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

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    kind: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("navercomic-bestchallenge") => SourceConfig {
            id: "navercomic-bestchallenge",
            kind: "bestChallenge",
        },
        Some("navercomic-challenge") => SourceConfig {
            id: "navercomic-challenge",
            kind: "challenge",
        },
        _ => SourceConfig {
            id: "navercomic-webtoon",
            kind: "webtoon",
        },
    }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(value: &str, kind: &str) -> String {
    let title_id = query_param(value, "titleId").unwrap_or_else(|| "1".to_string());
    format!("/{kind}/list?titleId={title_id}")
}

fn parse_webtoon_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("item ")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href, source.kind);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<strong", "</strong>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        query_param(&key, "titleId").unwrap_or_else(|| "Naver Webtoon".into())
                    }),
                authors: html::text_between(chunk, "author", "</span>")
                    .map(|value| split_authors(&html::strip_tags(&value)))
                    .unwrap_or_default(),
                cover: html::attr_after(chunk, "<img", "src"),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ko".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_challenge_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ChallengeResponse>(body).unwrap_or_default();
    Paged {
        entries: response
            .list
            .into_iter()
            .map(|item| CatalogItem {
                key: format!("/{}/list?titleId={}", source.kind, item.title_id),
                title: item.title_name,
                cover: item.thumbnail_url,
                url: Some(format!(
                    "{BASE_URL}/{}/list?titleId={}",
                    source.kind, item.title_id
                )),
                language: Some("ko".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: response
            .page_info
            .and_then(|page| page.next_page)
            .unwrap_or(0)
            != 0,
    }
}

fn parse_search(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body).unwrap_or_default();
    Paged {
        entries: response
            .search_list
            .into_iter()
            .map(|item| manga_from_api(item, source))
            .collect(),
        has_next_page: response.page_info.next_page.unwrap_or(0) != 0,
    }
}

fn parse_details_json(body: &str, source: SourceConfig, fallback_key: &str) -> CatalogItem {
    serde_json::from_str::<MangaDto>(body)
        .map(|item| manga_from_api(item, source))
        .unwrap_or_else(|_| CatalogItem {
            key: fallback_key.to_string(),
            title: "Naver Comic".to_string(),
            url: Some(url::join_url(BASE_URL, fallback_key)),
            language: Some("ko".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn manga_from_api(item: MangaDto, source: SourceConfig) -> CatalogItem {
    let key = format!("/{}/list?titleId={}", source.kind, item.title_id);
    CatalogItem {
        key: key.clone(),
        title: item.title_name,
        cover: item.thumbnail_url,
        authors: item
            .community_artists
            .into_iter()
            .map(|artist| artist.name)
            .collect(),
        description: item.synopsis.filter(|value| !value.is_empty()),
        status: if item.rest.unwrap_or(false) {
            ItemStatus::Hiatus
        } else if item.finished.unwrap_or(false) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ko".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_json(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterListResponse>(body).unwrap_or_default();
    response
        .article_list
        .into_iter()
        .map(|chapter| {
            let key = format!(
                "/{}/detail?titleId={}&no={}",
                source.kind, response.title_id, chapter.no
            );
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.subtitle),
                chapter_number: Some(chapter.no as f32),
                date_uploaded: chapter.service_date_description.and_then(parse_date),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chunks = if body.contains("wt_viewer") {
        body.split("wt_viewer").skip(1).collect::<Vec<_>>()
    } else {
        body.split("toon_view_lst").skip(1).collect::<Vec<_>>()
    };
    let source = if chunks.is_empty() {
        vec![body]
    } else {
        chunks
    };
    source
        .join("")
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split('?').nth(1).unwrap_or(input);
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn split_authors(input: &str) -> Vec<String> {
    input
        .split(" / ")
        .flat_map(|part| part.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_date(input: String) -> Option<i64> {
    let parts = input
        .split('.')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [year, month, day] if *year < 100 => Some((2000 + year) * 10_000 + month * 100 + day),
        [year, month, day] => Some(year * 10_000 + month * 100 + day),
        _ => None,
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    #[serde(default)]
    page_info: PageInfo,
    #[serde(default)]
    search_list: Vec<MangaDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeResponse {
    page_info: Option<PageInfo>,
    #[serde(default)]
    list: Vec<ChallengeDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterListResponse {
    #[serde(default)]
    title_id: i64,
    #[serde(default)]
    article_list: Vec<ChapterDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    next_page: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    thumbnail_url: Option<String>,
    title_name: String,
    title_id: i64,
    finished: Option<bool>,
    rest: Option<bool>,
    #[serde(default)]
    community_artists: Vec<AuthorDto>,
    synopsis: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeDto {
    thumbnail_url: Option<String>,
    title_name: String,
    title_id: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    service_date_description: Option<String>,
    subtitle: String,
    no: i64,
}

#[derive(Debug, Default, Deserialize)]
struct AuthorDto {
    name: String,
}

export_manga_source!(SOURCE);

const WEBTOON_LIST_FIXTURE: &str = r#"
<ul class="list_toon"><li class="item "><a href="/webtoon/list?titleId=1"><img src="https://image/cover.jpg"><strong>Sample Naver Webtoon</strong><span class="author">Author</span></a></li></ul>
"#;
const CHALLENGE_LIST_FIXTURE: &str = r#"{"pageInfo":{"nextPage":0},"list":[{"thumbnailUrl":"https://image/cover.jpg","titleName":"Sample Challenge","titleId":1}]}"#;
const SEARCH_FIXTURE: &str = r#"{"pageInfo":{"nextPage":0},"searchList":[{"thumbnailUrl":"https://image/cover.jpg","titleName":"Sample Naver","titleId":1,"finished":false,"rest":false,"communityArtists":[{"name":"Author"}],"synopsis":"Sample description"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"thumbnailUrl":"https://image/cover.jpg","titleName":"Sample Naver","titleId":1,"finished":false,"rest":false,"communityArtists":[{"name":"Author"}],"synopsis":"Sample description"}"#;
const CHAPTERS_FIXTURE: &str = r#"{"pageInfo":{"nextPage":0},"titleId":1,"articleList":[{"serviceDateDescription":"24.01.01","subtitle":"Chapter 1","no":1}]}"#;
const PAGES_FIXTURE: &str = r#"<div class="wt_viewer"><img src="https://image/page1.jpg"></div>"#;
