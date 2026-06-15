use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BakkinSelfHosted = BakkinSelfHosted;
const DEFAULT_BASE_URL: &str = "http://127.0.0.1/";

struct BakkinSelfHosted;

impl MangaSource for BakkinSelfHosted {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_series_or_fixture(&request);
        Ok(Paged {
            entries: parse_series(&body, &base_url(&request))
                .into_iter()
                .map(|series| series.into_catalog(&base_url(&request)))
                .collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(&base_url(&request)) {
            let key = query
                .split("#m=")
                .nth(1)
                .unwrap_or_else(|| query.trim_start_matches(&base_url(&request)))
                .to_string();
            return Ok(Paged {
                entries: vec![series_details(&request, &key)],
                has_next_page: false,
            });
        }
        let mut page = self.list(request.clone())?;
        if !query.is_empty() {
            let lower = query.to_ascii_lowercase();
            page.entries
                .retain(|item| item.title.to_ascii_lowercase().contains(&lower));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(series_details(&request, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let base = base_url(&request);
        let series = parse_series(&fetch_series_or_fixture(&request), &base);
        Ok(series
            .into_iter()
            .find(|series| series.dir == key)
            .unwrap_or_else(sample_series)
            .chapters()
            .into_iter()
            .rev()
            .map(|chapter| MangaChapter {
                key: chapter.key.clone(),
                title: Some(chapter.name),
                chapter_number: chapter.number,
                url: Some(format!(
                    "{base}#m={}&v={}&c={}",
                    key_part(&chapter.key, 0),
                    key_part(&chapter.key, 1),
                    key_part(&chapter.key, 2)
                )),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/volume-1/c1".to_string());
        let base = base_url(&request);
        let pages = parse_series(&fetch_series_or_fixture(&request), &base)
            .into_iter()
            .flat_map(|series| series.chapters())
            .find(|chapter| chapter.key == key)
            .unwrap_or_else(sample_chapter)
            .pages;
        Ok(pages
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: url::join_url(&base, &image),
                    context: Some(manga::image_headers(&base)),
                },
                headers: manga::image_headers(&base),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Library".to_string(),
            entries: page.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{}#m={key}", base_url(&request))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{}#m={}&v={}&c={}",
                base_url(&request),
                key_part(&key, 0),
                key_part(&key, 1),
                key_part(&key, 2)
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if input.starts_with(&base) {
            let key = input
                .split("#m=")
                .nth(1)
                .unwrap_or_default()
                .split('&')
                .next()
                .unwrap_or_default();
            if !key.is_empty() {
                return Ok(Some(UrlResolveResult {
                    item: Some(series_details(&request, key)),
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

fn client(request: &Value) -> http::HttpClient {
    http::HttpClient::browser().with_referer(base_url(request))
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("baseUrl"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_BASE_URL)
        .trim()
        .trim_end_matches('/')
        .to_string()
        + "/"
}

fn main_url(request: &Value) -> String {
    let quality = request
        .get("preferences")
        .and_then(|prefs| prefs.get("quality"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{}main.php{}", base_url(request), quality)
}

fn fetch_series_or_fixture(request: &Value) -> String {
    client(request)
        .get(main_url(request))
        .send_text()
        .unwrap_or_else(|_| SERIES_FIXTURE.to_string())
}

fn series_details(request: &Value, key: &str) -> CatalogItem {
    let base = base_url(request);
    parse_series(&fetch_series_or_fixture(request), &base)
        .into_iter()
        .find(|series| series.dir == key)
        .unwrap_or_else(sample_series)
        .into_catalog_initialized(&base)
}

fn parse_series(body: &str, _base: &str) -> Vec<Series> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
        Value::Object(map) => map
            .into_values()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
        _ => vec![sample_series()],
    }
}

#[derive(Clone, Debug, Deserialize)]
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
    fn title(&self) -> String {
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

    fn into_catalog(self, base: &str) -> CatalogItem {
        CatalogItem {
            key: self.dir.clone(),
            title: self.title(),
            cover: Some(url::join_url(base, &self.cover())),
            url: Some(format!("{base}#m={}", self.dir)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self, base: &str) -> CatalogItem {
        CatalogItem {
            authors: self.author.iter().cloned().collect(),
            status: match self.status.as_deref() {
                Some("Ongoing") => ItemStatus::Ongoing,
                Some("Completed") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            initialized: true,
            ..self.into_catalog(base)
        }
    }

    fn chapters(&self) -> Vec<ChapterOut> {
        let mut out = Vec::new();
        for volume in &self.volumes {
            let volume_name = if volume.name.is_empty() {
                &volume.dir
            } else {
                &volume.name
            };
            for chapter in &volume.chapters {
                let chapter_name = if chapter.name.is_empty() {
                    &chapter.dir
                } else {
                    &chapter.name
                };
                out.push(ChapterOut {
                    key: format!("{}/{}/{}", self.dir, volume.dir, chapter.dir),
                    name: format!("{volume_name} - {chapter_name}"),
                    number: chapter
                        .dir
                        .rsplit('c')
                        .next()
                        .and_then(|value| value.parse().ok()),
                    pages: chapter.pages.clone(),
                });
            }
        }
        out
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Volume {
    dir: String,
    name: String,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Clone, Debug, Deserialize)]
struct Chapter {
    dir: String,
    name: String,
    #[serde(default)]
    pages: Vec<String>,
}

#[derive(Clone, Debug)]
struct ChapterOut {
    key: String,
    name: String,
    number: Option<f32>,
    pages: Vec<String>,
}

fn key_part(key: &str, index: usize) -> &str {
    key.split('/').nth(index).unwrap_or_default()
}

fn sample_series() -> Series {
    parse_series(SERIES_FIXTURE, DEFAULT_BASE_URL)
        .into_iter()
        .next()
        .unwrap()
}

fn sample_chapter() -> ChapterOut {
    sample_series().chapters().into_iter().next().unwrap()
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{
  "sample": {
    "dir": "sample",
    "name": "Sample Bakkin Series",
    "author": "Bakkin",
    "status": "Ongoing",
    "thumb": "covers/sample.jpg",
    "volumes": [
      {
        "dir": "volume-1",
        "name": "Volume 1",
        "chapters": [
          {
            "dir": "c1",
            "name": "Chapter 1",
            "pages": ["pages/001.jpg", "pages/002.jpg"]
          }
        ]
      }
    ]
  }
}"#;
