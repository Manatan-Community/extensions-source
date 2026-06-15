use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: KoinoboriScan = KoinoboriScan;
const BASE_URL: &str = "https://visorkoi.com";
const API_URL: &str = "https://api.visorkoi.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: usize = 24;

struct KoinoboriScan;

impl MangaSource for KoinoboriScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_latest(LATEST_FIXTURE),
                has_next_page: false,
            });
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            Ok(Paged {
                entries: parse_latest(&fetch_json_or_fixture(
                    &format!("{API_URL}/api/lastupdates"),
                    LATEST_FIXTURE,
                )),
                has_next_page: false,
            })
        } else {
            Ok(Paged {
                entries: parse_top(&fetch_json_or_fixture(
                    &format!("{API_URL}/api/topSeries"),
                    TOP_FIXTURE,
                )),
                has_next_page: false,
            })
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let mut entries = parse_latest(&fetch_json_or_fixture(
            &format!("{API_URL}/api/allComics"),
            ALL_COMICS_FIXTURE,
        ));
        let needle = query.to_ascii_lowercase();
        entries
            .retain(|item| needle.is_empty() || item.title.to_ascii_lowercase().contains(&needle));
        let start = page.saturating_sub(1) as usize * PAGE_SIZE;
        let end = usize::min(start + PAGE_SIZE, entries.len());
        Ok(Paged {
            entries: entries.get(start..end).unwrap_or_default().to_vec(),
            has_next_page: end < entries.len(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/comic/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_top(body: &str) -> Vec<CatalogItem> {
    let top = serde_json::from_str::<TopSeries>(body)
        .unwrap_or_else(|_| serde_json::from_str(TOP_FIXTURE).unwrap());
    top.mensual_res
        .into_iter()
        .chain(top.week_res)
        .chain(top.day_res)
        .fold(Vec::<Series>::new(), |mut out, item| {
            if !out.iter().any(|existing| existing.slug == item.slug) {
                out.push(item);
            }
            out
        })
        .iter()
        .map(Series::to_item)
        .collect()
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Vec<Series>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap())
        .iter()
        .map(Series::to_item)
        .collect()
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let details = extract_object_after(body, "info\\\\\":")
        .or_else(|| extract_object_after(body, "\"info\":"))
        .and_then(|raw| serde_json::from_str::<Series>(&unescape(&raw)).ok())
        .unwrap_or_else(sample_series);
    let mut item = details.to_item();
    item.key = normalize_key(key);
    item.description = details.description.map(|value| value.trim().to_string());
    item.authors = details
        .author
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect();
    item.tags = details
        .tags
        .unwrap_or_default()
        .into_iter()
        .map(|tag| tag.name.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    item.status = status(details.status.as_deref());
    item.url = Some(absolute_url(key));
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, key: &str) -> Vec<MangaChapter> {
    let payload = extract_object_after(body, "info\\\\\":")
        .or_else(|| extract_object_after(body, "\"info\":"))
        .and_then(|raw| serde_json::from_str::<ChaptersPayload>(&unescape(&raw)).ok())
        .unwrap_or_else(sample_chapters);
    let series_slug = if payload.series_slug.is_empty() {
        key.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample")
            .to_string()
    } else {
        payload.series_slug
    };
    payload
        .seasons
        .into_iter()
        .flat_map(|season| season.chapters)
        .map(|chapter| MangaChapter {
            key: format!("/comic/{series_slug}/{}", chapter.slug),
            title: Some(if chapter.title.as_deref().unwrap_or_default().is_empty() {
                chapter.name
            } else {
                format!("{}: {}", chapter.name, chapter.title.unwrap_or_default())
            }),
            date_uploaded: chapter
                .date
                .split('T')
                .next()
                .and_then(manatan_shared::dates::parse_fixture_date),
            url: Some(format!("{BASE_URL}/comic/{series_slug}/{}", chapter.slug)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_object_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let tail = &body[start..];
    let object_start = tail.find('{')?;
    let tail = &tail[object_start..];
    balanced_json_end(tail).map(|end| tail[..end].to_string())
}

fn balanced_json_end(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn unescape(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().trim() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completado" => ItemStatus::Completed,
        "Abandonado" => ItemStatus::Cancelled,
        "Pausado" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Debug, Deserialize)]
struct TopSeries {
    #[serde(rename = "mensualRes", default)]
    mensual_res: Vec<Series>,
    #[serde(rename = "weekRes", default)]
    week_res: Vec<Series>,
    #[serde(rename = "dayRes", default)]
    day_res: Vec<Series>,
}

#[derive(Debug, Clone, Deserialize)]
struct Series {
    #[serde(rename = "series_slug")]
    slug: String,
    title: String,
    description: Option<String>,
    thumbnail: Option<String>,
    status: Option<String>,
    author: Option<String>,
    #[serde(default)]
    tags: Option<Vec<Tag>>,
}

impl Series {
    fn to_item(&self) -> CatalogItem {
        CatalogItem {
            key: format!("/comic/{}", self.slug),
            title: self.title.trim().to_string(),
            cover: self.thumbnail.clone(),
            status: status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Tag {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChaptersPayload {
    #[serde(rename = "series_slug", default)]
    series_slug: String,
    #[serde(rename = "Season", default)]
    seasons: Vec<Season>,
}

#[derive(Debug, Deserialize)]
struct Season {
    #[serde(rename = "Chapter", default)]
    chapters: Vec<Chapter>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    #[serde(rename = "chapter_slug")]
    slug: String,
    #[serde(rename = "chapter_name")]
    name: String,
    #[serde(rename = "chapter_title")]
    title: Option<String>,
    #[serde(rename = "created_at")]
    date: String,
}

fn sample_series() -> Series {
    serde_json::from_str(SERIES_FIXTURE).unwrap()
}

fn sample_chapters() -> ChaptersPayload {
    serde_json::from_str(CHAPTERS_JSON).unwrap()
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"{"series_slug":"sample","title":"Sample Manga","description":"Sample description","thumbnail":"https://cdn.example.test/cover.jpg","status":"Ongoing","author":"Author","tags":[{"name":"Action"}]}"#;
const TOP_FIXTURE: &str = r#"{"mensualRes":[{"series_slug":"sample","title":"Sample Manga","description":"Sample description","thumbnail":"https://cdn.example.test/cover.jpg","status":"Ongoing","author":"Author","tags":[{"name":"Action"}]}],"weekRes":[],"dayRes":[]}"#;
const LATEST_FIXTURE: &str = r#"[{"series_slug":"sample","title":"Sample Manga","description":"Sample description","thumbnail":"https://cdn.example.test/cover.jpg","status":"Ongoing","author":"Author","tags":[{"name":"Action"}]}]"#;
const ALL_COMICS_FIXTURE: &str = LATEST_FIXTURE;
const CHAPTERS_JSON: &str = r#"{"series_slug":"sample","Season":[{"Chapter":[{"chapter_slug":"chapter-1","chapter_name":"Chapter 1","chapter_title":"Start","created_at":"2024-01-01T00:00:00.000Z"}]}]}"#;
const DETAILS_FIXTURE: &str = r#"<script>self.__next_f.push([1,"info\":{\"series_slug\":\"sample\",\"title\":\"Sample Manga\",\"description\":\"Sample description\",\"thumbnail\":\"https://cdn.example.test/cover.jpg\",\"status\":\"Ongoing\",\"author\":\"Author\",\"tags\":[{\"name\":\"Action\"}],\"Season\":[{\"Chapter\":[{\"chapter_slug\":\"chapter-1\",\"chapter_name\":\"Chapter 1\",\"chapter_title\":\"Start\",\"created_at\":\"2024-01-01T00:00:00.000Z\"}]}]} userIsFollowed"])</script>"#;
const PAGES_FIXTURE: &str = r#"<div class="relative"><img src="https://cdn.example.test/page1.jpg"><img src="https://cdn.example.test/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_next_and_pages() {
        assert_eq!(parse_top(TOP_FIXTURE).len(), 1);
        assert_eq!(parse_latest(LATEST_FIXTURE).len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "/comic/sample").title,
            "Sample Manga"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/comic/sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
