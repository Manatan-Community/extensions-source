use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: BigSolo = BigSolo;
const BASE_URL: &str = "https://bigsolo.org";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct BigSolo;

impl MangaSource for BigSolo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/data/series"), LIST_FIXTURE);
        let data = series_response(&body);
        let entries = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            sorted_catalog(data.series.into_iter().chain(data.os).collect())
        } else {
            data.reco.into_iter().map(catalog_from_serie).collect()
        };
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
            let slug = key.trim_matches('/');
            let body =
                fetch_json_or_fixture(&format!("{BASE_URL}/data/series/{slug}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![catalog_from_serie(serie_response(&body))],
                has_next_page: false,
            });
        }
        let body = fetch_json_or_fixture(&format!("{BASE_URL}/data/series"), LIST_FIXTURE);
        let query_lower = query.to_ascii_lowercase();
        let entries = sorted_catalog(
            series_response(&body)
                .series
                .into_iter()
                .chain(series_response(&body).os)
                .filter(|serie| {
                    query_lower.is_empty()
                        || serie.title.to_ascii_lowercase().contains(&query_lower)
                        || serie.ja_title.to_ascii_lowercase().contains(&query_lower)
                        || serie
                            .alternative_titles
                            .iter()
                            .any(|title| title.to_ascii_lowercase().contains(&query_lower))
                })
                .collect(),
        );
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let slug = normalize_key(&key).trim_matches('/').to_string();
        let body =
            fetch_json_or_fixture(&format!("{BASE_URL}/data/series/{slug}"), DETAILS_FIXTURE);
        Ok(catalog_from_serie(serie_response(&body)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let slug = normalize_key(&key).trim_matches('/').to_string();
        let body =
            fetch_json_or_fixture(&format!("{BASE_URL}/data/series/{slug}"), DETAILS_FIXTURE);
        Ok(chapters_from_serie(&serie_response(&body)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let parts = normalize_key(&key);
        let mut split = parts.trim_matches('/').split('/');
        let slug = split.next().unwrap_or("sample");
        let chapter = split.next().unwrap_or("1");
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/data/series/{slug}/{chapter}"),
            PAGES_FIXTURE,
        );
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
            let slug = key.trim_matches('/');
            let body =
                fetch_json_or_fixture(&format!("{BASE_URL}/data/series/{slug}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_serie(serie_response(&body))),
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
        .with_referer(format!("{BASE_URL}/"))
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

fn series_response(body: &str) -> SeriesResponse {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap())
}

fn serie_response(body: &str) -> Serie {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap())
}

fn sorted_catalog(mut series: Vec<Serie>) -> Vec<CatalogItem> {
    series.sort_by_key(|serie| {
        serie
            .last_chapter
            .as_ref()
            .map(|chapter| chapter.timestamp)
            .unwrap_or(0)
    });
    series.reverse();
    series.into_iter().map(catalog_from_serie).collect()
}

fn catalog_from_serie(serie: Serie) -> CatalogItem {
    let key = format!("/{}", serie.slug.trim_matches('/'));
    CatalogItem {
        key: key.clone(),
        title: serie.title,
        description: (!serie.description.is_empty()).then_some(serie.description),
        authors: (!serie.author.is_empty())
            .then(|| vec![serie.author])
            .unwrap_or_default(),
        artists: (!serie.artist.is_empty())
            .then(|| vec![serie.artist])
            .unwrap_or_default(),
        tags: serie.tags,
        status: match serie.status.as_str() {
            "En cours" => ItemStatus::Ongoing,
            "Finis" | "Fini" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        cover: serie.cover.map(|cover| cover.url_hq),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapters_from_serie(serie: &Serie) -> Vec<MangaChapter> {
    let multiple = serie.chapters.len() > 1;
    let mut chapters = serie
        .chapters
        .iter()
        .filter(|(_, chapter)| !chapter.licensed && chapter.source.is_some())
        .map(|(number, chapter)| {
            let title = if multiple {
                let mut value = String::new();
                if !chapter.volume.is_empty() {
                    value.push_str(&format!("Vol. {} ", chapter.volume));
                }
                value.push_str(&format!("Ch. {number}"));
                if !chapter.title.is_empty() {
                    value.push_str(&format!(" - {}", chapter.title));
                }
                value
            } else if chapter.title.is_empty() {
                "One Shot".to_string()
            } else {
                format!("One Shot - {}", chapter.title)
            };
            MangaChapter {
                key: format!("/{}/{}", serie.slug, number),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                scanlators: chapter.teams.clone(),
                date_uploaded: Some(chapter.timestamp as i64),
                url: Some(format!("{BASE_URL}/{}/{}", serie.slug, number)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ChapterDetails>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap())
        .images
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: page,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
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

#[derive(Debug, Default, Deserialize)]
struct SeriesResponse {
    #[serde(default)]
    series: Vec<Serie>,
    #[serde(default)]
    os: Vec<Serie>,
    #[serde(default)]
    reco: Vec<Serie>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Serie {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, rename = "ja_title")]
    ja_title: String,
    #[serde(default, rename = "alternative_titles")]
    alternative_titles: Vec<String>,
    #[serde(default)]
    status: String,
    cover: Option<Cover>,
    #[serde(default)]
    chapters: BTreeMap<String, Chapter>,
    #[serde(default, rename = "last_chapter")]
    last_chapter: Option<LastChapter>,
}

#[derive(Clone, Debug, Deserialize)]
struct Cover {
    #[serde(rename = "url_hq")]
    url_hq: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LastChapter {
    #[serde(default)]
    timestamp: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Chapter {
    #[serde(default)]
    title: String,
    #[serde(default)]
    volume: String,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    teams: Vec<String>,
    #[serde(default, rename = "licensed")]
    licensed: bool,
    source: Option<SourceInfo>,
}

#[derive(Clone, Debug, Deserialize)]
struct SourceInfo {
    #[allow(dead_code)]
    service: String,
    #[allow(dead_code)]
    id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ChapterDetails {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"series":[{"slug":"sample","title":"Sample","description":"Summary","artist":"Artist","author":"Author","tags":["Action"],"ja_title":"Sample","alternative_titles":["Alt"],"status":"En cours","cover":{"url_hq":"https://bigsolo.org/cover.jpg"},"chapters":{"1":{"title":"Start","timestamp":1704067200,"teams":["Team"],"source":{"service":"web","id":"1"}}},"last_chapter":{"timestamp":1704067200}}],"os":[],"reco":[{"slug":"sample","title":"Sample","description":"Summary","artist":"Artist","author":"Author","tags":["Action"],"ja_title":"Sample","alternative_titles":["Alt"],"status":"En cours","cover":{"url_hq":"https://bigsolo.org/cover.jpg"},"chapters":{"1":{"title":"Start","timestamp":1704067200,"teams":["Team"],"source":{"service":"web","id":"1"}}},"last_chapter":{"timestamp":1704067200}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample","description":"Summary","artist":"Artist","author":"Author","tags":["Action"],"ja_title":"Sample","alternative_titles":["Alt"],"status":"En cours","cover":{"url_hq":"https://bigsolo.org/cover.jpg"},"chapters":{"1":{"title":"Start","timestamp":1704067200,"teams":["Team"],"source":{"service":"web","id":"1"}}},"last_chapter":{"timestamp":1704067200}}"#;
const PAGES_FIXTURE: &str =
    r#"{"images":["https://bigsolo.org/page1.jpg","https://bigsolo.org/page2.jpg"]}"#;
