use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: ScanR = ScanR;
const BASE_URL: &str = "https://teamscanr.fr";
const CDN_URL: &str = "https://cdn.teamscanr.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "adult";

struct ScanR;

impl MangaSource for ScanR {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_index(LIST_FIXTURE, "", request.get("filters")));
        }
        Ok(parse_index(
            &fetch_text(&format!("{CDN_URL}/index.json"), LIST_FIXTURE),
            "",
            request.get("filters"),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let query = slug_query(query).unwrap_or_else(|| query.to_string());
        Ok(parse_index(
            &fetch_text(&format!("{CDN_URL}/index.json"), LIST_FIXTURE),
            &query,
            request.get("filters"),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let slug = key.trim_matches('/').split('/').next().unwrap_or("sample");
        let index = fetch_index();
        let filename = index
            .get(slug)
            .cloned()
            .unwrap_or_else(|| "sample.json".into());
        Ok(fetch_series(&filename).to_item(true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let slug = key.trim_matches('/').split('/').next().unwrap_or("sample");
        let index = fetch_index();
        let filename = index
            .get(slug)
            .cloned()
            .unwrap_or_else(|| "sample.json".into());
        Ok(series_chapters(&fetch_series(&filename)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
        let slug = parts.first().copied().unwrap_or("sample");
        let chapter_id = parts.get(1).copied().unwrap_or("1").replace('-', ".");
        let index = fetch_index();
        let filename = index
            .get(slug)
            .cloned()
            .unwrap_or_else(|| "sample.json".into());
        let series = fetch_series(&filename);
        let Some(chapter) = series.chapters.get(&chapter_id) else {
            return Ok(Vec::new());
        };
        let Some(proxy) = chapter.groups.values().next() else {
            return Ok(Vec::new());
        };
        Ok(parse_pages(&fetch_text(
            &format!("https://cubari.moe{proxy}"),
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
        if let Some(slug) = slug_query(input) {
            let index = fetch_index();
            if let Some(filename) = index.get(&slug) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_series(filename).to_item(true)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_index() -> BTreeMap<String, String> {
    serde_json::from_str(&fetch_text(&format!("{CDN_URL}/index.json"), LIST_FIXTURE))
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .unwrap_or_default()
}

fn fetch_series(filename: &str) -> Series {
    serde_json::from_str(&fetch_text(
        &format!("{CDN_URL}/{filename}"),
        SERIES_FIXTURE,
    ))
    .or_else(|_| serde_json::from_str(SERIES_FIXTURE))
    .unwrap_or_default()
}

fn parse_index(body: &str, query: &str, filters: Option<&Value>) -> Paged<CatalogItem> {
    let index = serde_json::from_str::<BTreeMap<String, String>>(body)
        .or_else(|_| serde_json::from_str(LIST_FIXTURE))
        .unwrap_or_default();
    let query = query.trim();
    let mut entries = Vec::new();
    for (slug, filename) in index {
        if query.starts_with("SLUG:") && query.trim_start_matches("SLUG:") != slug {
            continue;
        }
        let series = fetch_series(&filename);
        if !query.is_empty()
            && !query.starts_with("SLUG:")
            && !series
                .title
                .to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
        {
            continue;
        }
        if !matches_filters(&series, filters) {
            continue;
        }
        entries.push(series.to_item(query.starts_with("SLUG:")));
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn matches_filters(series: &Series, filters: Option<&Value>) -> bool {
    let types = filter_values(filters, "type", &["os", "series"]);
    let statuses = filter_values(filters, "status", &["completed", "ongoing"]);
    let adults = filter_values(filters, "adult", &["18", "normal"]);
    let status = if series.os || series.completed {
        "completed"
    } else {
        "ongoing"
    };
    let ty = if series.os { "os" } else { "series" };
    let adult = if series.konami { "18" } else { "normal" };
    types.iter().any(|value| value == ty)
        && statuses.iter().any(|value| value == status)
        && adults.iter().any(|value| value == adult)
}

fn filter_values(filters: Option<&Value>, id: &str, default: &[&str]) -> Vec<String> {
    match filters.and_then(|filters| filters.get(id)) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if value.contains(',') => value
            .split(',')
            .map(|part| part.trim().to_string())
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.to_string()],
        _ => default.iter().map(|value| value.to_string()).collect(),
    }
}

fn series_chapters(series: &Series) -> Vec<MangaChapter> {
    let mut chapters = series
        .chapters
        .iter()
        .map(|(number, chapter)| {
            let title = if series.os {
                if chapter.title.trim().is_empty() {
                    "One Shot".to_string()
                } else {
                    format!("One Shot - {}", chapter.title.trim())
                }
            } else {
                let mut name = String::new();
                if !chapter.volume.trim().is_empty() {
                    name.push_str(&format!("Vol. {} ", chapter.volume.trim()));
                }
                name.push_str(&format!("Ch. {number}"));
                if !chapter.title.trim().is_empty() {
                    name.push_str(&format!(" - {}", chapter.title.trim()));
                }
                name
            };
            let key = format!("/{}/{}", series.slug, number.replace('.', "-"));
            MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                date_uploaded: chapter
                    .last_updated
                    .parse::<i64>()
                    .ok()
                    .map(|seconds| seconds * 1000),
                scanlators: chapter.groups.keys().next().cloned().into_iter().collect(),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .unwrap_or(-1.0)
            .total_cmp(&a.chapter_number.unwrap_or(-1.0))
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Vec<String>>(body)
        .or_else(|_| serde_json::from_str(PAGES_FIXTURE))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn slug_query(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    input
        .trim_start_matches(BASE_URL)
        .trim_matches('/')
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

impl Series {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        let title = if self.konami {
            format!("[+18] {}", self.title)
        } else {
            self.title.clone()
        };
        let key = format!("/{}", self.slug);
        CatalogItem {
            key: key.clone(),
            title,
            description: non_empty(&self.description),
            authors: non_empty(&self.author).into_iter().collect(),
            artists: non_empty(&self.artist).into_iter().collect(),
            cover: non_empty(&self.cover),
            status: if self.os || self.completed {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[derive(Debug, Default, Deserialize)]
struct Series {
    slug: String,
    title: String,
    description: String,
    artist: String,
    author: String,
    cover: String,
    #[serde(default)]
    os: bool,
    #[serde(default)]
    chapters: BTreeMap<String, Chapter>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    konami: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Chapter {
    title: String,
    volume: String,
    #[serde(rename = "last_updated")]
    last_updated: String,
    groups: BTreeMap<String, String>,
}

const LIST_FIXTURE: &str = r#"{"sample":"sample.json"}"#;
const SERIES_FIXTURE: &str = r#"
{
  "slug": "sample",
  "title": "Sample ScanR",
  "description": "Resume",
  "artist": "Artist",
  "author": "Author",
  "cover": "https://cdn.teamscanr.fr/cover.jpg",
  "os": false,
  "completed": false,
  "konami": false,
  "chapters": {
    "1": {
      "title": "Debut",
      "volume": "",
      "last_updated": "1704067200",
      "groups": { "ScanR": "/read/imgur/sample/1" }
    }
  }
}
"#;
const PAGES_FIXTURE: &str = r#"["https://cdn.teamscanr.fr/page1.jpg"]"#;
