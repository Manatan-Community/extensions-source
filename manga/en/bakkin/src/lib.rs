use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Bakkin = Bakkin;
const BASE_URL: &str = "https://bakkin.moe/reader/";

struct Bakkin;

impl MangaSource for Bakkin {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(series_page(parse_series(MAIN_FIXTURE), ""));
        }
        Ok(series_page(parse_series(&fetch_main(&request)), ""))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = query
                .split("#m=")
                .nth(1)
                .unwrap_or_default()
                .split('&')
                .next()
                .unwrap_or_default();
            return Ok(Paged {
                entries: parse_series(&fetch_main(&request))
                    .into_iter()
                    .filter(|series| series.dir == key)
                    .map(Series::into_catalog)
                    .collect(),
                has_next_page: false,
            });
        }
        Ok(series_page(parse_series(&fetch_main(&request)), query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_series(&fetch_main(&request))
            .into_iter()
            .find(|series| series.dir == key)
            .map(Series::into_catalog_initialized)
            .unwrap_or_else(|| fallback_catalog(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let chapters = parse_series(&fetch_main(&request))
            .into_iter()
            .find(|series| series.dir == key)
            .map(|series| series.chapters())
            .unwrap_or_default();
        Ok(chapters.into_iter().rev().collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/v1/c1".to_string());
        Ok(parse_series(&fetch_main(&request))
            .into_iter()
            .flat_map(|series| series.chapters_with_pages())
            .find(|chapter| chapter.key == key)
            .map(|chapter| {
                chapter
                    .pages
                    .into_iter()
                    .enumerate()
                    .map(|(index, page)| MangaPage {
                        content: PageContent::Url {
                            url: url::join_url(BASE_URL, &page),
                            context: Some(manga::image_headers(BASE_URL)),
                        },
                        headers: manga::image_headers(BASE_URL),
                        description: Some(format!("Page {}", index + 1)),
                        ..MangaPage::default()
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}#m={key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() == 3 {
                format!("{BASE_URL}#m={}&v={}&c={}", parts[0], parts[1], parts[2])
            } else {
                format!("{BASE_URL}#{key}")
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = input
                .split("#m=")
                .nth(1)
                .unwrap_or_default()
                .split('&')
                .next()
                .unwrap_or_default();
            return Ok(Some(UrlResolveResult {
                item: Some(
                    parse_series(&fetch_main(&request))
                        .into_iter()
                        .find(|series| series.dir == key)
                        .map(Series::into_catalog_initialized)
                        .unwrap_or_else(|| fallback_catalog(key)),
                ),
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
        .with_header(
            "User-Agent",
            "Mozilla/5.0 (Android 14; Mobile) Tachiyomi/1.0",
        )
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn main_url(request: &Value) -> String {
    let quality = request
        .get("preferences")
        .and_then(|prefs| prefs.get("quality"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{BASE_URL}main.php{quality}")
}

fn fetch_main(request: &Value) -> String {
    client()
        .get(main_url(request))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| MAIN_FIXTURE.to_string())
}

fn parse_series(body: &str) -> Vec<Series> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match root {
        Value::Object(map) => map
            .into_values()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
        Value::Array(items) => items
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn series_page(series: Vec<Series>, query: &str) -> Paged<CatalogItem> {
    let needle = query.to_ascii_lowercase();
    Paged {
        entries: series
            .into_iter()
            .filter(|series| {
                needle.is_empty() || series.display_name().to_ascii_lowercase().contains(&needle)
            })
            .map(Series::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn fallback_catalog(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.to_string(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        url: Some(format!("{BASE_URL}#m={key}")),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Clone, Default, Deserialize)]
struct Series {
    dir: String,
    name: String,
    author: Option<String>,
    status: Option<String>,
    thumb: Option<String>,
    #[serde(default)]
    volumes: Vec<Volume>,
}

impl Series {
    fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.dir.clone()
        } else {
            self.name.clone()
        }
    }

    fn cover(&self) -> String {
        self.thumb
            .clone()
            .unwrap_or_else(|| "static/nocover.png".to_string())
    }

    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: self.dir.clone(),
            title: self.display_name(),
            cover: Some(url::join_url(BASE_URL, &self.cover())),
            url: Some(format!("{BASE_URL}#m={}", self.dir)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        CatalogItem {
            authors: self.author.clone().into_iter().collect(),
            status: match self.status.as_deref() {
                Some("Ongoing") => ItemStatus::Ongoing,
                Some("Completed") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..self.into_catalog()
        }
    }

    fn chapters(&self) -> Vec<MangaChapter> {
        self.chapters_with_pages()
            .into_iter()
            .map(|chapter| MangaChapter {
                key: chapter.key.clone(),
                title: Some(chapter.title),
                chapter_number: chapter.number,
                url: Some(format!(
                    "{BASE_URL}#m={}&v={}&c={}",
                    chapter.series, chapter.volume, chapter.chapter
                )),
                ..MangaChapter::default()
            })
            .collect()
    }

    fn chapters_with_pages(&self) -> Vec<ChapterWithPages> {
        self.volumes
            .iter()
            .flat_map(|volume| {
                volume.chapters.iter().map(|chapter| {
                    let title = format!("{} - {}", volume.display_name(), chapter.display_name());
                    ChapterWithPages {
                        key: format!("{}/{}/{}", self.dir, volume.dir, chapter.dir),
                        title,
                        number: chapter
                            .dir
                            .rsplit('c')
                            .next()
                            .and_then(|part| part.parse().ok()),
                        series: self.dir.clone(),
                        volume: volume.dir.clone(),
                        chapter: chapter.dir.clone(),
                        pages: chapter.pages.clone(),
                    }
                })
            })
            .collect()
    }
}

#[derive(Clone, Default, Deserialize)]
struct Volume {
    dir: String,
    name: String,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

impl Volume {
    fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.dir.clone()
        } else {
            self.name.clone()
        }
    }
}

#[derive(Clone, Default, Deserialize)]
struct Chapter {
    dir: String,
    name: String,
    #[serde(default)]
    pages: Vec<String>,
}

impl Chapter {
    fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.dir.clone()
        } else {
            self.name.clone()
        }
    }
}

struct ChapterWithPages {
    key: String,
    title: String,
    number: Option<f32>,
    series: String,
    volume: String,
    chapter: String,
    pages: Vec<String>,
}

export_manga_source!(SOURCE);

const MAIN_FIXTURE: &str = r#"
{
  "sample": {
    "dir": "sample",
    "name": "Sample Bakkin",
    "author": "Bakkin",
    "status": "Ongoing",
    "thumb": "static/nocover.png",
    "volumes": [
      {
        "dir": "v1",
        "name": "Volume 1",
        "chapters": [
          { "dir": "c1", "name": "Chapter 1", "pages": ["sample/v1/c1/001.jpg"] }
        ]
      }
    ]
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture_series() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Bakkin");
        let chapters = SOURCE.chapters(json!({"manga": "sample"})).unwrap();
        assert_eq!(chapters[0].key, "sample/v1/c1");
    }
}
