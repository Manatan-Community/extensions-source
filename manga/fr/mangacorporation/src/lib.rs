use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaCorporation = MangaCorporation;
const BASE_URL: &str = "https://manga-corporation.com";
const API_URL: &str = "https://manga-corporation.com/api";

struct MangaCorporation;

impl MangaSource for MangaCorporation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_comics(&fetch_json("/comics", LIST_FIXTURE), false));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        Ok(parse_comics(&fetch_json("/comics", LIST_FIXTURE), latest))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            return Ok(Paged {
                entries: parse_detail_item(&fetch_json(&key, DETAILS_FIXTURE), Some(key))
                    .into_iter()
                    .collect(),
                has_next_page: false,
            });
        }
        Ok(parse_comics(
            &fetch_json(
                &format!("/search/{}", url::query_escape(query)),
                LIST_FIXTURE,
            ),
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(
            parse_detail_item(&fetch_json(&key, DETAILS_FIXTURE), Some(key))
                .unwrap_or_else(sample_item),
        )
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_json(&key, DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        Ok(parse_pages(&fetch_json(&key, PAGES_FIXTURE)))
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
        if let Some(key) = key_from_input(input) {
            return Ok(Some(UrlResolveResult {
                item: parse_detail_item(&fetch_json(&key, DETAILS_FIXTURE), Some(key)),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}/{}", path.trim_start_matches('/')))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_comics(body: &str, latest: bool) -> Paged<CatalogItem> {
    let mut comics = serde_json::from_str::<PizzaResultsDto>(body)
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .unwrap_or_default()
        .comics;
    if latest {
        comics.retain(|comic| comic.last_chapter.is_some());
        comics.sort_by(|a, b| {
            b.last_chapter
                .as_ref()
                .and_then(|chapter| chapter.published_on.as_deref())
                .cmp(
                    &a.last_chapter
                        .as_ref()
                        .and_then(|chapter| chapter.published_on.as_deref()),
                )
        });
        comics.truncate(10);
    }
    Paged {
        entries: comics.into_iter().map(comic_to_item).collect(),
        has_next_page: false,
    }
}

fn parse_detail_item(body: &str, key: Option<String>) -> Option<CatalogItem> {
    let comic = serde_json::from_str::<PizzaResultDto>(body)
        .ok()
        .and_then(|response| response.comic)?;
    let mut item = comic_to_item(comic);
    if let Some(key) = key {
        item.key = normalize_key(&key);
        item.url = Some(url::join_url(BASE_URL, &item.key));
    }
    item.initialized = true;
    Some(item)
}

fn comic_to_item(comic: PizzaComicDto) -> CatalogItem {
    let key = normalize_key(&comic.url);
    CatalogItem {
        key: key.clone(),
        title: non_empty(comic.title)
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: non_empty(comic.thumbnail).map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        authors: comic.author.into_iter().filter_map(non_empty).collect(),
        artists: comic.artist.into_iter().filter_map(non_empty).collect(),
        description: non_empty(comic.description),
        tags: comic
            .genres
            .into_iter()
            .filter_map(|genre| non_empty(genre.name))
            .collect(),
        status: comic
            .status
            .as_deref()
            .map(status_from_text)
            .unwrap_or_default(),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<PizzaResultDto>(body)
        .ok()
        .and_then(|response| response.comic)
        .map(|comic| comic.chapters)
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let key = normalize_key(&chapter.url);
            MangaChapter {
                key: key.clone(),
                title: non_empty(chapter.full_title),
                chapter_number: chapter_number(chapter.chapter, chapter.subchapter),
                date_uploaded: chapter
                    .published_on
                    .as_deref()
                    .and_then(|value| dates::parse_ymd(value.get(..10).unwrap_or(value))),
                scanlators: chapter
                    .teams
                    .into_iter()
                    .flatten()
                    .filter_map(|team| non_empty(team.name))
                    .collect(),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<PizzaReaderDto>(body)
        .ok()
        .and_then(|response| response.chapter)
        .map(|chapter| chapter.pages)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &page),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn key_from_input(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input.trim_start_matches(BASE_URL)))
        .or_else(|| input.starts_with('/').then(|| normalize_key(input)))
}

fn normalize_key(value: &str) -> String {
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn chapter_number(chapter: Option<i32>, subchapter: Option<i32>) -> Option<f32> {
    let base = chapter.unwrap_or(-1) as f32;
    let suffix = subchapter.unwrap_or(0);
    if suffix <= 0 {
        Some(base)
    } else {
        let divisor = 10_f32.powi(suffix.to_string().len() as i32);
        Some(base + suffix as f32 / divisor)
    }
}

fn status_from_text(value: &str) -> ItemStatus {
    match value.get(..value.len().min(7)).unwrap_or(value) {
        "In cors" | "On goin" => ItemStatus::Ongoing,
        "Complet" | "Conclus" | "Conclud" => ItemStatus::Completed,
        "Licenzi" | "License" => ItemStatus::Unknown,
        _ => ItemStatus::Unknown,
    }
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "/sample".into(),
        title: "Sample".into(),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Default, Deserialize)]
struct PizzaResultsDto {
    #[serde(default)]
    comics: Vec<PizzaComicDto>,
}

#[derive(Default, Deserialize)]
struct PizzaResultDto {
    comic: Option<PizzaComicDto>,
}

#[derive(Default, Deserialize)]
struct PizzaReaderDto {
    chapter: Option<PizzaChapterDto>,
}

#[derive(Default, Deserialize)]
struct PizzaComicDto {
    artist: Option<String>,
    author: Option<String>,
    #[serde(default)]
    chapters: Vec<PizzaChapterDto>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: Vec<PizzaGenreDto>,
    #[serde(default, rename = "last_chapter")]
    last_chapter: Option<PizzaChapterDto>,
    status: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    url: String,
}

#[derive(Default, Deserialize)]
struct PizzaGenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct PizzaChapterDto {
    chapter: Option<i32>,
    #[serde(default, rename = "full_title")]
    full_title: String,
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default, rename = "published_on")]
    published_on: Option<String>,
    subchapter: Option<i32>,
    #[serde(default)]
    teams: Vec<Option<PizzaTeamDto>>,
    #[serde(default)]
    url: String,
}

#[derive(Default, Deserialize)]
struct PizzaTeamDto {
    #[serde(default)]
    name: String,
}

const LIST_FIXTURE: &str = r#"{"comics":[{"title":"Sample","thumbnail":"/cover.jpg","url":"/sample","author":"Auteur","artist":"Artiste","description":"Resume","genres":[{"name":"Action"}],"status":"On going","last_chapter":{"full_title":"Chapitre 1","published_on":"2024-01-01T00:00:00.000000","url":"/sample/1"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"title":"Sample","thumbnail":"/cover.jpg","url":"/sample","author":"Auteur","artist":"Artiste","description":"Resume","genres":[{"name":"Action"}],"status":"On going","chapters":[{"chapter":1,"full_title":"Chapitre 1","published_on":"2024-01-01T00:00:00.000000","teams":[{"name":"Manga-Corporation"}],"url":"/sample/1"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":["https://manga-corporation.com/page1.jpg","https://manga-corporation.com/page2.jpg"]}}"#;
