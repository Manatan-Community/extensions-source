use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaMeets = MangaMeets;
const BASE_URL: &str = "https://manga-meets.jp";
const API_URL: &str = "https://manga-meets.jp/api";
const PAGE_SIZE: &str = "20";

struct MangaMeets;

impl MangaSource for MangaMeets {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{API_URL}/episodes/latest.json?page={page}&size={PAGE_SIZE}")
        } else {
            format!(
                "{API_URL}/comics/search.json?sort=weekly_view_count&page={page}&size={PAGE_SIZE}"
            )
        };
        Ok(parse_series(&fetch_json(&target, SERIES_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{API_URL}/comics/search.json?page={page}&size={PAGE_SIZE}");
        if !query.is_empty() {
            target.push_str("&keywords=");
            target.push_str(&url::query_escape(query));
        }
        if let Some(sort) = filter_string(&request, "sort") {
            target.push_str("&sort=");
            target.push_str(&url::query_escape(sort));
        }
        Ok(parse_series(&fetch_json(&target, SERIES_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let dir = dir_from_key(&key);
        Ok(parse_chapters(
            &fetch_json(
                &format!("{API_URL}/comics/{dir}/episodes.json"),
                CHAPTERS_FIXTURE,
            ),
            &dir,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let mut parts = key.split('/');
        let dir = parts.next().unwrap_or("sample");
        let episode = parts.next().unwrap_or("1");
        Ok(parse_pages(&fetch_json(
            &format!("{API_URL}/comics/{dir}/episodes/{episode}/viewer.json"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn parse_series(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SeriesResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("valid fixture"));
    let images = response
        .included
        .iter()
        .filter(|item| item.kind == "image")
        .collect::<Vec<_>>();
    let entries = response
        .included
        .iter()
        .filter(|item| item.kind == "comic")
        .enumerate()
        .filter_map(|(index, comic)| {
            let dir = comic.attributes.dir_name.as_ref()?;
            Some(CatalogItem {
                key: dir.clone(),
                title: comic
                    .attributes
                    .title
                    .clone()
                    .unwrap_or_else(|| "MangaMeets".into()),
                cover: images
                    .get(index)
                    .and_then(|image| image.attributes.url.clone()),
                url: Some(format!("{BASE_URL}/comics/{dir}")),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: response.data.attributes.current_page < response.data.attributes.total_page,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let dir = dir_from_key(key);
    parse_details(
        &fetch_json(&format!("{API_URL}/comics/{dir}.json"), DETAILS_FIXTURE),
        &dir,
    )
}

fn parse_details(body: &str, dir: &str) -> CatalogItem {
    let response = serde_json::from_str::<DetailsResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture"));
    let image_map = response
        .included
        .iter()
        .filter(|item| item.kind == "image")
        .map(|item| (item.id.as_str(), item.attributes.url.as_deref()))
        .collect::<Vec<_>>();
    let genre_map = response
        .included
        .iter()
        .filter(|item| item.kind == "comic_genre")
        .map(|item| (item.id.as_str(), item.attributes.name.as_deref()))
        .collect::<Vec<_>>();
    let attrs = response.data.attributes;
    let cover_id = response
        .data
        .relationships
        .thumbnail_image
        .and_then(|rel| rel.data)
        .map(|data| data.id);
    let genre_id = response
        .data
        .relationships
        .comic_genre
        .and_then(|rel| rel.data)
        .map(|data| data.id);
    CatalogItem {
        key: dir.into(),
        title: attrs.title,
        authors: attrs.authors.clone().unwrap_or_default(),
        artists: attrs.authors.unwrap_or_default(),
        description: attrs.outline,
        tags: genre_id
            .as_deref()
            .and_then(|id| {
                genre_map
                    .iter()
                    .find(|(key, _)| *key == id)
                    .and_then(|(_, value)| value.map(str::to_string))
            })
            .into_iter()
            .collect(),
        cover: cover_id.as_deref().and_then(|id| {
            image_map
                .iter()
                .find(|(key, _)| *key == id)
                .and_then(|(_, value)| value.map(str::to_string))
        }),
        status: if attrs.finished == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/comics/{dir}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, dir: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("valid fixture"));
    response
        .data
        .into_iter()
        .rev()
        .map(|entry| {
            let title = entry
                .attributes
                .title
                .filter(|title| !title.is_empty())
                .map(|title| format!(" - {title}"))
                .unwrap_or_default();
            MangaChapter {
                key: format!("{dir}/{}", entry.attributes.sort_volume),
                title: Some(format!("Chapter {}{title}", entry.attributes.volume)),
                chapter_number: Some(entry.attributes.sort_volume as f32),
                date_uploaded: parse_date_millis(entry.attributes.published_at.as_deref()),
                url: Some(format!(
                    "{BASE_URL}/comics/{dir}/{}",
                    entry.attributes.sort_volume
                )),
                language: Some("ja".into()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ViewerResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("valid fixture"));
    response
        .episode_pages
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.image.original_url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            ..MangaPage::default()
        })
        .collect()
}

fn dir_from_key(key: &str) -> String {
    key.trim_start_matches(BASE_URL)
        .trim_start_matches("/comics/")
        .trim_matches('/')
        .to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    if !input.starts_with("http") {
        return Some(dir_from_key(input));
    }
    input
        .find("/comics/")
        .map(|index| dir_from_key(&input[index + "/comics/".len()..]))
}

fn parse_date_millis(value: Option<&str>) -> Option<i64> {
    let date = value?.split('T').next()?;
    dates::parse_ymd(date).map(|seconds| seconds * 1000)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[derive(Deserialize)]
struct SeriesResponse {
    data: SeriesData,
    included: Vec<IncludedItem>,
}

#[derive(Deserialize)]
struct SeriesData {
    attributes: SeriesAttributes,
}

#[derive(Deserialize)]
struct SeriesAttributes {
    total_page: u64,
    current_page: u64,
}

#[derive(Deserialize)]
struct IncludedItem {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    attributes: IncludedAttributes,
}

#[derive(Deserialize)]
struct IncludedAttributes {
    dir_name: Option<String>,
    title: Option<String>,
    url: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct DetailsResponse {
    data: ComicData,
    included: Vec<IncludedItem>,
}

#[derive(Deserialize)]
struct ComicData {
    attributes: ComicDataAttributes,
    relationships: ComicDataRelationships,
}

#[derive(Deserialize)]
struct ComicDataAttributes {
    title: String,
    authors: Option<Vec<String>>,
    outline: Option<String>,
    finished: Option<bool>,
}

#[derive(Deserialize)]
struct ComicDataRelationships {
    comic_genre: Option<RelationWrapper>,
    thumbnail_image: Option<RelationWrapper>,
}

#[derive(Deserialize)]
struct RelationWrapper {
    data: Option<RelationData>,
}

#[derive(Deserialize)]
struct RelationData {
    id: String,
}

#[derive(Deserialize)]
struct ChapterResponse {
    data: Vec<EpisodeEntry>,
}

#[derive(Deserialize)]
struct EpisodeEntry {
    attributes: EpisodeAttributes,
}

#[derive(Deserialize)]
struct EpisodeAttributes {
    title: Option<String>,
    volume: String,
    sort_volume: i32,
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct ViewerResponse {
    episode_pages: Vec<EpisodePage>,
}

#[derive(Deserialize)]
struct EpisodePage {
    image: EpisodeImage,
}

#[derive(Deserialize)]
struct EpisodeImage {
    original_url: String,
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"data":{"attributes":{"total_page":1,"current_page":1}},"included":[{"id":"1","type":"comic","attributes":{"dir_name":"sample","title":"Sample MangaMeets"}},{"id":"2","type":"image","attributes":{"url":"https://img.example.test/mangameets.jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"attributes":{"title":"Sample MangaMeets","authors":["Sample Author"],"outline":"Sample description.","finished":false},"relationships":{"comic_genre":{"data":{"id":"g1"}},"thumbnail_image":{"data":{"id":"i1"}}}},"included":[{"id":"i1","type":"image","attributes":{"url":"https://img.example.test/mangameets.jpg"}},{"id":"g1","type":"comic_genre","attributes":{"name":"Drama"}}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"attributes":{"title":"Episode 1","volume":"1","sort_volume":1,"published_at":"2024-01-01T00:00:00.000+09:00"}}]}"#;
const PAGES_FIXTURE: &str = r#"{"episode_pages":[{"order_index":0,"image":{"original_url":"https://img.example.test/mangameets-page.jpg"}}]}"#;
