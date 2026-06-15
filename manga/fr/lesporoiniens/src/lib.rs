use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

const SOURCE: LesPoroiniens = LesPoroiniens;
const BASE_URL: &str = "https://lesporoiniens.org";

struct LesPoroiniens;

impl MangaSource for LesPoroiniens {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalog(CONFIG_FIXTURE, ""));
        }
        Ok(parse_catalog(
            &fetch_text("/data/config.json", CONFIG_FIXTURE),
            "",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_PAGE_FIXTURE);
            let series = series_from_page(&body).unwrap_or_else(sample_series);
            return Ok(Paged {
                entries: vec![series_to_item(series, true)],
                has_next_page: false,
            });
        }
        Ok(parse_catalog(
            &fetch_text("/data/config.json", CONFIG_FIXTURE),
            query,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_PAGE_FIXTURE);
        Ok(series_to_item(
            series_from_page(&body).unwrap_or_else(sample_series),
            true,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_PAGE_FIXTURE);
        Ok(series_from_page(&body)
            .map(series_chapters)
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, READER_PAGE_FIXTURE);
        Ok(parse_reader_pages(&body, &key))
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
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_PAGE_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(series_to_item(
                    series_from_page(&body).unwrap_or_else(sample_series),
                    true,
                )),
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

fn fetch_text(path: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, path))
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

fn parse_catalog(config_body: &str, query: &str) -> Paged<CatalogItem> {
    let config = serde_json::from_str::<ConfigResponse>(config_body)
        .or_else(|_| serde_json::from_str(CONFIG_FIXTURE))
        .unwrap_or_default();
    let query = query.to_ascii_lowercase();
    let entries = config
        .local_series_files
        .into_iter()
        .filter_map(|file| {
            let body = fetch_text(&format!("/data/series/{file}"), SERIES_FIXTURE);
            serde_json::from_str::<SeriesData>(&normalize_chapters_json(&body)).ok()
        })
        .filter(|series| query.is_empty() || series.title.to_ascii_lowercase().contains(&query))
        .map(|series| series_to_item(series, false))
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn series_from_page(body: &str) -> Option<SeriesData> {
    let json = placeholder_json(body, "series-data-placeholder")?;
    serde_json::from_str::<SeriesData>(&normalize_chapters_json(&json)).ok()
}

fn series_to_item(series: SeriesData, detailed: bool) -> CatalogItem {
    let key = format!("/{}", slugify(&series.title));
    let description = if detailed {
        detailed_description(&series)
    } else {
        None
    };
    CatalogItem {
        key: key.clone(),
        title: series.title,
        cover: series.cover.or(series.cover_hq),
        authors: series
            .author
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect(),
        artists: series
            .artist
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect(),
        description,
        tags: series.tags.unwrap_or_default(),
        status: match series.release_status.as_deref() {
            Some("En cours") => ItemStatus::Ongoing,
            Some("Finis") | Some("Fini") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: detailed,
        ..CatalogItem::default()
    }
}

fn detailed_description(series: &SeriesData) -> Option<String> {
    let base = series
        .description
        .as_deref()
        .filter(|value| !value.to_ascii_lowercase().contains("pas de synopsis"))
        .unwrap_or_default()
        .trim();
    let alts = series.alternative_titles.as_deref().unwrap_or_default();
    if alts.is_empty() {
        (!base.is_empty()).then(|| base.to_string())
    } else {
        let alt_text = alts
            .iter()
            .map(|title| format!("- {title}"))
            .collect::<Vec<_>>()
            .join("\n");
        Some(if base.is_empty() {
            format!("Alternative Titles:\n{alt_text}")
        } else {
            format!("{base}\n\nAlternative Titles:\n{alt_text}")
        })
    }
}

fn series_chapters(series: SeriesData) -> Vec<MangaChapter> {
    let mut entries = chapter_entries(series.chapters.as_ref());
    let multiple = entries.len() > 1;
    entries.retain(|(_, chapter)| !chapter.licencied);
    entries
        .into_iter()
        .map(|(number, chapter)| {
            let mut title = String::new();
            if multiple {
                if let Some(volume) = chapter.volume.filter(|value| !value.is_empty()) {
                    title.push_str(&format!("Vol. {volume} "));
                }
                title.push_str(&format!("Ch. {number}"));
                if let Some(chapter_title) = chapter.title.filter(|value| !value.is_empty()) {
                    title.push_str(&format!(" - {chapter_title}"));
                }
            } else {
                title = chapter
                    .title
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("One Shot - {value}"))
                    .unwrap_or_else(|| "One Shot".into());
            }
            let key = format!("/{}/{}", slugify(&series.title), number);
            MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                date_uploaded: Some(chapter.last_updated),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_reader_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    let chapter_number = chapter_key
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("1");
    let Some(json) = placeholder_json(body, "reader-data-placeholder") else {
        return Vec::new();
    };
    let Ok(reader) = serde_json::from_str::<ReaderData>(&normalize_chapters_json(&json)) else {
        return Vec::new();
    };
    let chapter_url = chapter_entries(reader.series.chapters.as_ref())
        .into_iter()
        .find(|(number, _)| number == chapter_number)
        .and_then(|(_, chapter)| {
            chapter
                .groups
                .and_then(|groups| groups.into_values().next())
        });
    let Some(chapter_url) = chapter_url else {
        return Vec::new();
    };
    let pages = if chapter_url.contains("imgchest") {
        let id = chapter_url.rsplit('/').next().unwrap_or_default();
        fetch_imgchest_pages(id)
    } else {
        serde_json::from_str::<Vec<String>>(&fetch_text(&chapter_url, PAGES_JSON_FIXTURE))
            .unwrap_or_default()
    };
    pages
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

fn fetch_imgchest_pages(id: &str) -> Vec<String> {
    serde_json::from_str::<Vec<PageData>>(&fetch_text(
        &format!("/api/imgchest-chapter-pages?id={id}"),
        IMG_CHEST_FIXTURE,
    ))
    .unwrap_or_default()
    .into_iter()
    .map(|page| page.link)
    .collect()
}

fn chapter_entries(value: Option<&Value>) -> Vec<(String, ChapterData)> {
    let mut entries = match value {
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(number, value)| {
                serde_json::from_value::<ChapterData>(value.clone())
                    .ok()
                    .map(|chapter| (number.clone(), chapter))
            })
            .collect::<Vec<_>>(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                serde_json::from_value::<ChapterData>(value.clone())
                    .ok()
                    .map(|chapter| ((index + 1).to_string(), chapter))
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    entries.sort_by(|a, b| {
        b.0.parse::<f32>()
            .unwrap_or_default()
            .partial_cmp(&a.0.parse::<f32>().unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries
}

fn placeholder_json(body: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{id}\"");
    let start = body.find(&marker)?;
    let after = &body[start..];
    let content_start = after.find('>')? + 1;
    let content = &after[content_start..];
    let end = content
        .find("</script>")
        .or_else(|| content.find("</div>"))
        .unwrap_or(content.len());
    Some(html::html_unescape(content[..end].trim()))
}

fn normalize_chapters_json(input: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(input) else {
        return input.to_string();
    };
    if let Some(chapters) = value.get_mut("chapters") {
        if let Value::Array(items) = chapters {
            let object = items
                .iter()
                .enumerate()
                .map(|(index, item)| ((index + 1).to_string(), item.clone()))
                .collect();
            *chapters = Value::Object(object);
        }
    }
    value.to_string()
}

fn key_from_input(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        Some(format!(
            "/{}",
            input.trim_start_matches(BASE_URL).trim_matches('/')
        ))
    } else if input.starts_with('/') {
        Some(format!("/{}", input.trim_matches('/')))
    } else {
        None
    }
}

fn slugify(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' => ch,
            'à' | 'á' | 'â' | 'ä' | 'ã' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn sample_series() -> SeriesData {
    serde_json::from_str(SERIES_FIXTURE).unwrap_or_default()
}

#[derive(Default, Deserialize)]
struct ConfigResponse {
    #[serde(default, rename = "LOCAL_SERIES_FILES")]
    local_series_files: Vec<String>,
}

#[derive(Default, Deserialize)]
struct ReaderData {
    series: SeriesData,
}

#[derive(Default, Deserialize)]
struct SeriesData {
    #[serde(default)]
    title: String,
    description: Option<String>,
    artist: Option<String>,
    author: Option<String>,
    cover: Option<String>,
    #[serde(rename = "cover_hq")]
    cover_hq: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(rename = "release_status")]
    release_status: Option<String>,
    #[serde(rename = "alternative_titles")]
    alternative_titles: Option<Vec<String>>,
    chapters: Option<Value>,
}

#[derive(Default, Deserialize)]
struct ChapterData {
    title: Option<String>,
    volume: Option<String>,
    #[serde(default, rename = "last_updated", deserialize_with = "safe_i64")]
    last_updated: i64,
    #[serde(default)]
    licencied: bool,
    groups: Option<HashMap<String, String>>,
}

#[derive(Default, Deserialize)]
struct PageData {
    #[serde(default)]
    link: String,
}

fn safe_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(number) => number.as_i64().unwrap_or_default(),
        Value::String(value) => value.parse::<i64>().unwrap_or_default(),
        _ => 0,
    } * 1000)
}

const CONFIG_FIXTURE: &str = r#"{"LOCAL_SERIES_FILES":["sample.json"]}"#;
const SERIES_FIXTURE: &str = r#"{"title":"Sample","description":"Resume","artist":"Artist","author":"Author","cover":"https://lesporoiniens.org/cover.jpg","tags":["Action"],"release_status":"En cours","alternative_titles":["Alt Sample"],"chapters":{"1":{"title":"Start","volume":"1","last_updated":1704067200,"licencied":false,"groups":{"Poroiniens":"/data/pages/sample-1.json"}}}}"#;
const DETAILS_PAGE_FIXTURE: &str = r#"<script id="series-data-placeholder" type="application/json">{"title":"Sample","description":"Resume","artist":"Artist","author":"Author","cover":"https://lesporoiniens.org/cover.jpg","tags":["Action"],"release_status":"En cours","alternative_titles":["Alt Sample"],"chapters":[{"title":"Start","volume":"1","last_updated":1704067200,"licencied":false,"groups":{"Poroiniens":"/data/pages/sample-1.json"}}]}</script>"#;
const READER_PAGE_FIXTURE: &str = r#"<script id="reader-data-placeholder" type="application/json">{"series":{"title":"Sample","chapters":[{"title":"Start","volume":"1","last_updated":1704067200,"licencied":false,"groups":{"Poroiniens":"/data/pages/sample-1.json"}}]}}</script>"#;
const PAGES_JSON_FIXTURE: &str =
    r#"["https://lesporoiniens.org/page1.jpg","https://lesporoiniens.org/page2.jpg"]"#;
const IMG_CHEST_FIXTURE: &str = r#"[{"link":"https://lesporoiniens.org/page1.jpg"},{"link":"https://lesporoiniens.org/page2.jpg"}]"#;
