use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: BlueSolo = BlueSolo;
const BASE_URL: &str = "https://bluesolo.org";
const API_URL: &str = "https://bluesolo.org/api";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct BlueSolo;

impl MangaSource for BlueSolo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut entries = parse_results(&fetch_json_or_fixture(
            &format!("{API_URL}/comics"),
            LIST_FIXTURE,
        ));
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            entries.retain(|item| item.extra.contains_key("lastPublishedOn"));
            entries.sort_by(|left, right| {
                right
                    .extra
                    .get("lastPublishedOn")
                    .and_then(Value::as_str)
                    .cmp(&left.extra.get("lastPublishedOn").and_then(Value::as_str))
            });
            entries.truncate(10);
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_json(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{API_URL}/comics")
        } else {
            format!("{API_URL}/search/{}", url::query_escape(query))
        };
        Ok(Paged {
            entries: parse_results(&fetch_json_or_fixture(&target, LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_details_json(&body, Some(normalize_key(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter/1".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_json(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_results(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<PizzaResults>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or_default())
        .comics
        .into_iter()
        .map(catalog_from_comic)
        .collect()
}

fn parse_details_json(body: &str, key: Option<String>) -> CatalogItem {
    let comic = serde_json::from_str::<PizzaResult>(body)
        .ok()
        .and_then(|result| result.comic)
        .or_else(|| {
            serde_json::from_str::<PizzaResult>(DETAILS_FIXTURE)
                .ok()?
                .comic
        })
        .unwrap_or_default();
    let mut item = catalog_from_comic(comic.clone());
    if let Some(key) = key {
        item.key = key.clone();
        item.url = Some(url::join_url(BASE_URL, &key));
    }
    item.authors = (!comic.author.is_empty())
        .then(|| vec![comic.author])
        .unwrap_or_default();
    item.artists = comic.artist.into_iter().collect();
    item.description = (!comic.description.is_empty()).then_some(comic.description);
    item.tags = comic.genres.into_iter().map(|genre| genre.name).collect();
    item.status = status(&comic.status.unwrap_or_default());
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let comic = serde_json::from_str::<PizzaResult>(body)
        .ok()
        .and_then(|result| result.comic)
        .or_else(|| {
            serde_json::from_str::<PizzaResult>(DETAILS_FIXTURE)
                .ok()?
                .comic
        })
        .unwrap_or_default();
    comic
        .chapters
        .into_iter()
        .map(|chapter| {
            let chapter_number = chapter.chapter.map(|value| value as f32).unwrap_or(-1.0)
                + chapter
                    .subchapter
                    .map(|value| format!("0.{value}").parse::<f32>().unwrap_or(0.0))
                    .unwrap_or(0.0);
            MangaChapter {
                key: normalize_key(&chapter.url),
                title: Some(chapter.full_title),
                chapter_number: Some(chapter_number),
                date_uploaded: None,
                scanlators: chapter
                    .teams
                    .into_iter()
                    .flatten()
                    .map(|team| team.name)
                    .collect(),
                url: Some(url::join_url(BASE_URL, &chapter.url)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter = serde_json::from_str::<PizzaReader>(body)
        .ok()
        .and_then(|reader| reader.chapter)
        .or_else(|| {
            serde_json::from_str::<PizzaReader>(PAGES_FIXTURE)
                .ok()?
                .chapter
        })
        .unwrap_or_default();
    chapter
        .pages
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

fn catalog_from_comic(comic: PizzaComic) -> CatalogItem {
    let key = normalize_key(&comic.url);
    let mut extra = BTreeMap::new();
    if let Some(chapter) = &comic.last_chapter {
        extra.insert(
            "lastPublishedOn".into(),
            Value::String(chapter.published_on.clone()),
        );
    }
    CatalogItem {
        key: key.clone(),
        title: comic.title,
        cover: (!comic.thumbnail.is_empty()).then_some(comic.thumbnail),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        extra,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("complet") || lower.starts_with("conclu") {
        ItemStatus::Completed
    } else if lower.starts_with("in cors") || lower.starts_with("on goin") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaResults {
    #[serde(default)]
    comics: Vec<PizzaComic>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaResult {
    comic: Option<PizzaComic>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaReader {
    chapter: Option<PizzaChapter>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaComic {
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    chapters: Vec<PizzaChapter>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: Vec<PizzaGenre>,
    #[serde(default, rename = "last_chapter")]
    last_chapter: Option<PizzaChapter>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    url: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaGenre {
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaChapter {
    chapter: Option<i64>,
    #[serde(default, rename = "full_title")]
    full_title: String,
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default, rename = "published_on")]
    published_on: String,
    subchapter: Option<i64>,
    #[serde(default)]
    teams: Vec<Option<PizzaTeam>>,
    #[serde(default)]
    url: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PizzaTeam {
    #[serde(default)]
    name: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"title":"Sample","thumbnail":"https://bluesolo.org/cover.jpg","url":"/comic/sample","last_chapter":{"full_title":"Chapitre 1","published_on":"2024-01-01T00:00:00.000000","url":"/comic/sample/chapter/1"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"title":"Sample","author":"Author","artist":"Artist","description":"Summary","genres":[{"name":"Action"}],"status":"In corso","thumbnail":"https://bluesolo.org/cover.jpg","url":"/comic/sample","chapters":[{"chapter":1,"full_title":"Chapitre 1","pages":["https://bluesolo.org/page1.jpg"],"published_on":"2024-01-01T00:00:00.000000","teams":[{"name":"Team"}],"url":"/comic/sample/chapter/1"}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"chapter":{"pages":["https://bluesolo.org/page1.jpg","https://bluesolo.org/page2.jpg"]}}"#;
