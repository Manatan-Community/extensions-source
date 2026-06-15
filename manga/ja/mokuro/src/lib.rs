use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Mokuro = Mokuro;
const BASE_URL: &str = "https://mokuro.moe";
const API_BASE_URL: &str = "https://mokuro.moe/catalog/api";

struct Mokuro;

impl MangaSource for Mokuro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let library = fetch_library();
        let use_latest_cover = preference_bool(&request, "useLatestVolumeCover")
            || preference_bool(&request, "pref_use_latest_volume_cover");
        Ok(Paged {
            entries: library
                .series
                .into_iter()
                .map(|series| series.into_catalog(use_latest_cover))
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
        if let Some(path) = path_from_url(query) {
            let library = fetch_library();
            if let Some(series) = library
                .series
                .into_iter()
                .find(|series| series.path == path)
            {
                return Ok(Paged {
                    entries: vec![
                        series.into_catalog(preference_bool(&request, "useLatestVolumeCover")),
                    ],
                    has_next_page: false,
                });
            }
        }
        let needle = query.to_lowercase();
        let use_latest_cover = preference_bool(&request, "useLatestVolumeCover")
            || preference_bool(&request, "pref_use_latest_volume_cover");
        Ok(Paged {
            entries: fetch_library()
                .series
                .into_iter()
                .filter(|series| needle.is_empty() || series.name.to_lowercase().contains(&needle))
                .map(|series| series.into_catalog(use_latest_cover))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-series".into());
        let use_latest_cover = preference_bool(&request, "useLatestVolumeCover")
            || preference_bool(&request, "pref_use_latest_volume_cover");
        Ok(fetch_library()
            .series
            .into_iter()
            .find(|series| series.path == key)
            .map(|series| series.into_catalog(use_latest_cover))
            .unwrap_or_else(|| fallback_catalog(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-series".into());
        let Some(series) = fetch_library()
            .series
            .into_iter()
            .find(|series| series.path == key)
        else {
            return Ok(Vec::new());
        };
        Ok(series
            .volumes
            .into_iter()
            .rev()
            .map(|volume| volume.into_chapter(&series.path))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample-series|Volume 1".into());
        let (series_path, volume_name) = key.split_once('|').unwrap_or((&key, "Volume 1"));
        let mokuro_url = mokuro_reader_url(series_path, volume_name, "mokuro");
        let cbz_url = mokuro_reader_url(series_path, volume_name, "cbz");
        let payload = fetch_json(&mokuro_url, MOKURO_FIXTURE);
        let mokuro = serde_json::from_str::<MokuroDto>(&payload)
            .unwrap_or_else(|_| serde_json::from_str(MOKURO_FIXTURE).unwrap());
        Ok(mokuro
            .pages
            .into_iter()
            .map(|page| manga::archive_page(&cbz_url, &page.img_path))
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/catalog#{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (series_path, volume_name) = key.split_once('|').unwrap_or((&key, "Volume 1"));
            let cbz_url = mokuro_reader_url(series_path, volume_name, "cbz");
            format!(
                "https://reader.mokuro.app/#/upload?cbz={}",
                query_escape_strict(&cbz_url)
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(
                    fetch_library()
                        .series
                        .into_iter()
                        .find(|series| series.path == path)
                        .map(|series| series.into_catalog(false))
                        .unwrap_or_else(|| fallback_catalog(&path)),
                ),
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
        .with_referer(format!("{BASE_URL}/catalog"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_library() -> LibraryDto {
    serde_json::from_str(&fetch_json(
        &format!("{API_BASE_URL}/library"),
        LIBRARY_FIXTURE,
    ))
    .unwrap_or_else(|_| serde_json::from_str(LIBRARY_FIXTURE).unwrap())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Referer", format!("{BASE_URL}/catalog"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn mokuro_reader_url(series_path: &str, volume_name: &str, extension: &str) -> String {
    format!(
        "{BASE_URL}/mokuro-reader/{}/{}.{}",
        path_segment_escape(series_path),
        path_segment_escape(volume_name),
        extension
    )
}

fn cover_url(path: &str) -> String {
    format!("{API_BASE_URL}/cover?path={}", url::query_escape(path))
}

fn path_segment_escape(input: &str) -> String {
    input
        .as_bytes()
        .iter()
        .map(|byte| match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn query_escape_strict(input: &str) -> String {
    input
        .as_bytes()
        .iter()
        .map(|byte| match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/catalog#"))
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_chapter_number(name: &str) -> Option<f32> {
    let mut last = None;
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            current.push(ch);
        } else if !current.is_empty() {
            last = current.parse::<f32>().ok();
            current.clear();
        }
    }
    if !current.is_empty() {
        last = current.parse::<f32>().ok();
    }
    last
}

fn fallback_catalog(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: key.replace(['-', '_'], " "),
        url: Some(format!("{BASE_URL}/catalog#{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Default, Deserialize)]
struct LibraryDto {
    #[serde(default)]
    series: Vec<SeriesDto>,
}

#[derive(Default, Deserialize)]
struct SeriesDto {
    name: String,
    path: String,
    cover: Option<String>,
    #[serde(default)]
    volumes: Vec<VolumeDto>,
}

impl SeriesDto {
    fn into_catalog(self, use_latest_cover: bool) -> CatalogItem {
        let selected_cover = if use_latest_cover {
            self.volumes
                .last()
                .and_then(|volume| volume.cover.clone())
                .or(self.cover)
        } else {
            self.cover
        };
        CatalogItem {
            key: self.path.clone(),
            title: self.name,
            cover: selected_cover.as_deref().map(cover_url),
            url: Some(format!("{BASE_URL}/catalog#{}", self.path)),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct VolumeDto {
    name: String,
    cover: Option<String>,
}

impl VolumeDto {
    fn into_chapter(self, series_path: &str) -> MangaChapter {
        MangaChapter {
            key: format!("{series_path}|{}", self.name),
            title: Some(self.name.clone()),
            chapter_number: parse_chapter_number(&self.name),
            url: Some(format!(
                "https://reader.mokuro.app/#/upload?cbz={}",
                query_escape_strict(&mokuro_reader_url(series_path, &self.name, "cbz"))
            )),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct MokuroDto {
    #[serde(default)]
    pages: Vec<MokuroPageDto>,
}

#[derive(Default, Deserialize)]
struct MokuroPageDto {
    #[serde(rename = "img_path")]
    img_path: String,
}

const LIBRARY_FIXTURE: &str = r#"
{"series":[{"name":"Sample Mokuro","path":"sample-series","cover":"sample/cover.jpg","volumes":[{"name":"Volume 1","cover":"sample/vol1.jpg"}]}]}
"#;

const MOKURO_FIXTURE: &str = r#"
{"pages":[{"img_path":"001.jpg"},{"img_path":"002.jpg"}]}
"#;

export_manga_source!(SOURCE);
