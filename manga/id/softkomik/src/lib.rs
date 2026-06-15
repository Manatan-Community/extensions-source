use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Softkomik = Softkomik;
const BASE_URL: &str = "https://softkomik.co";
const API_URL: &str = "https://v2.softdevices.my.id";
const COVER_URL: &str = "https://cover.softdevices.my.id/softkomik-cover";
const PRIMARY_CDN: &str = "https://psy1.komik.im";
const SECONDARY_CDN: &str = "https://cdn1.softkomik.online/softkomik";
const REQUIRED_LOGIN_FRAGMENT: &str = "#login-required";
const CONTENT_RATING: &str = "safe";

struct Softkomik;

impl MangaSource for Softkomik {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "popular"
        } else {
            "newKomik"
        };
        Ok(parse_library(&fetch_rsc_or_fixture(
            &library_url(page, Some(sort), request.get("filters")),
            LIST_FIXTURE,
        )))
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_library(&fetch_api_or_fixture(
                &format!(
                    "{API_URL}/komik?name={}&search=true&limit=20&page={page}",
                    url::query_escape(query)
                ),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_library(&fetch_rsc_or_fixture(
            &library_url(page, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let details = details_by_key(&key);
        let mature = details.tags.iter().any(|tag| {
            tag.eq_ignore_ascii_case("ecchi") || tag.eq_ignore_ascii_case("mature")
        });
        Ok(parse_chapters(
            &fetch_api_or_fixture(
                &format!(
                    "{API_URL}/komik/{}/chapter?limit=9999999",
                    slug_from_key(&key)
                ),
                CHAPTERS_FIXTURE,
            ),
            &key,
            mature,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter/1".into());
        let target = url::join_url(BASE_URL, key.trim_end_matches(REQUIRED_LOGIN_FRAGMENT));
        let page = fetch_rsc_or_fixture(&target, PAGES_FIXTURE);
        let Some(data) = extract_next::<ChapterPageData>(&page, "_id") else {
            return Ok(vec![manga::text_page(
                "Softkomik chapter data was not found in the reader response.",
            )]);
        };
        let images = if data.image_src.is_empty() {
            let Some(chapter) = chapter_from_key(&key) else {
                return Ok(Vec::new());
            };
            fetch_api_images(&slug_from_key(&key), &chapter, &data.id, key.contains(REQUIRED_LOGIN_FRAGMENT))
        } else {
            data.image_src
        };
        if images.is_empty() {
            return Ok(vec![manga::text_page(
                "This chapter returned no public page images. Open the source in WebView and sign in if the chapter is age-restricted.",
            )]);
        }
        let image_base = if data.storage_inter2.unwrap_or(false) {
            SECONDARY_CDN
        } else {
            PRIMARY_CDN
        };
        Ok(images
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let url = cdn_url(image_base, &image);
                let mut headers = manga::image_headers(BASE_URL);
                headers.insert("Origin".to_string(), BASE_URL.to_string());
                MangaPage {
                    content: PageContent::Url { url, context: None },
                    headers,
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&normalize_key(input))),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .header("rsc", "1")
        .header("RSC", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    let mut headers = Headers::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    client()
        .fetch("GET", target, None, headers)
        .ok()
        .and_then(|response| response.text)
        .unwrap_or_else(|| fixture.to_string())
}

fn fetch_api_images(slug: &str, chapter: &str, id: &str, login_required: bool) -> Vec<String> {
    if login_required {
        return Vec::new();
    }
    let target = format!("{API_URL}/komik/{slug}/chapter/{chapter}/img/{id}");
    let body = fetch_api_or_fixture(&target, r#"{"imageSrc":[]}"#);
    serde_json::from_str::<ChapterPageImages>(&body)
        .map(|data| data.image_src)
        .unwrap_or_default()
}

fn library_url(page: u64, sort: Option<&str>, filters: Option<&Value>) -> String {
    let mut params = vec![format!("page={page}")];
    for id in ["status", "type", "genre", "min"] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{id}={}", url::query_escape(&value)));
        }
    }
    let sort = filter_string(filters, "sortBy")
        .filter(|value| !value.is_empty())
        .or_else(|| sort.map(ToString::to_string));
    if let Some(value) = sort {
        params.push(format!("sortBy={}", url::query_escape(&value)));
    }
    format!("{BASE_URL}/komik/library?{}", params.join("&"))
}

fn filter_string(filters: Option<&Value>, id: &str) -> Option<String> {
    filters?
        .get(id)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_library(body: &str) -> Paged<CatalogItem> {
    let data = extract_next::<LibData>(body, "maxPage")
        .or_else(|| serde_json::from_str::<LibData>(body).ok())
        .unwrap_or_default();
    Paged {
        entries: data
            .data
            .into_iter()
            .map(|item| CatalogItem {
                key: normalize_key(&item.title_slug),
                title: item.title,
                cover: Some(cover_url(&item.gambar)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&item.title_slug))),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: data.page < data.max_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_rsc_or_fixture(&url::join_url(BASE_URL, key), DETAILS_FIXTURE);
    let data = extract_next::<MangaDetails>(&body, "sinopsis")
        .or_else(|| serde_json::from_str::<MangaDetails>(&body).ok())
        .unwrap_or_else(|| MangaDetails::fixture(slug_from_key(key)));
    CatalogItem {
        key: normalize_key(key),
        title: data.title,
        cover: Some(cover_url(&data.gambar)),
        authors: data.author.into_iter().collect(),
        description: data.sinopsis.map(|text| html::strip_tags(&text)),
        tags: data.genre.unwrap_or_default(),
        status: match data.status.unwrap_or_default().to_ascii_lowercase().as_str() {
            "ongoing" => ItemStatus::Ongoing,
            "tamat" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str, login_required: bool) -> Vec<MangaChapter> {
    let slug = slug_from_key(manga_key);
    let mut chapters = serde_json::from_str::<ChapterList>(body)
        .unwrap_or_default()
        .chapter
        .into_iter()
        .map(|chapter| {
            let chapter_number = chapter
                .chapter
                .split('.')
                .next()
                .and_then(|value| value.parse::<f32>().ok());
            let display = format_chapter_display(&chapter.chapter);
            let mut key = format!("/{slug}/chapter/{}", chapter.chapter);
            if login_required {
                key.push_str(REQUIRED_LOGIN_FRAGMENT);
            }
            MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapter {display}")),
                chapter_number,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn format_chapter_display(value: &str) -> String {
    let Some((number, suffix)) = value.split_once('.') else {
        return value.to_string();
    };
    if suffix.is_empty() {
        number.to_string()
    } else {
        format!("{}.{}", number.trim_start_matches('0').max("0"), suffix)
    }
}

fn cover_url(path: &str) -> String {
    format!("{}/{}", COVER_URL.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn cdn_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn normalize_key(value: &str) -> String {
    let path = value
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/');
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with("komik/") {
        format!("/{}", path.trim_end_matches('/'))
    } else {
        format!("/{}", path.trim_end_matches('/'))
    }
}

fn slug_from_key(key: &str) -> String {
    normalize_key(key)
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn chapter_from_key(key: &str) -> Option<String> {
    let clean = normalize_key(key);
    let mut segments = clean.trim_start_matches('/').split('/');
    let _slug = segments.next()?;
    if segments.next()? != "chapter" {
        return None;
    }
    segments.next().map(ToString::to_string)
}

fn extract_next<T: serde::de::DeserializeOwned>(body: &str, marker: &str) -> Option<T> {
    if let Ok(value) = serde_json::from_str(body) {
        return Some(value);
    }
    for candidate in balanced_json_values(body)
        .into_iter()
        .filter(|item| item.contains(marker))
    {
        if let Ok(value) = serde_json::from_str(&candidate) {
            return Some(value);
        }
        let unescaped = candidate.replace("\\\"", "\"").replace("\\\\", "\\");
        if let Ok(value) = serde_json::from_str(&unescaped) {
            return Some(value);
        }
    }
    None
}

fn balanced_json_values(body: &str) -> Vec<String> {
    let mut values = Vec::new();
    for (start, ch) in body
        .char_indices()
        .filter(|(_, ch)| *ch == '{' || *ch == '[')
    {
        if let Some(end) = matching_json_end(body, start, ch) {
            values.push(body[start..=end].to_string());
        }
    }
    values
}

fn matching_json_end(body: &str, start: usize, opening: char) -> Option<usize> {
    let mut stack = vec![opening];
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in body[start + opening.len_utf8()..].char_indices() {
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
            '{' | '[' => stack.push(ch),
            '}' => {
                if stack.pop() != Some('{') {
                    return None;
                }
            }
            ']' => {
                if stack.pop() != Some('[') {
                    return None;
                }
            }
            _ => {}
        }
        if stack.is_empty() {
            return Some(start + opening.len_utf8() + offset);
        }
    }
    None
}

#[derive(Default, Deserialize)]
struct LibData {
    data: Vec<MangaDto>,
    #[serde(rename = "maxPage", default)]
    max_page: u64,
    #[serde(default)]
    page: u64,
}

#[derive(Deserialize)]
struct MangaDto {
    gambar: String,
    title: String,
    title_slug: String,
}

#[derive(Deserialize)]
struct MangaDetails {
    gambar: String,
    title: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(rename = "Genre", default)]
    genre: Option<Vec<String>>,
    #[serde(default)]
    sinopsis: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

impl MangaDetails {
    fn fixture(slug: String) -> Self {
        Self {
            gambar: "/sample.jpg".to_string(),
            title: slug.replace('-', " "),
            author: None,
            genre: Some(Vec::new()),
            sinopsis: None,
            status: None,
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterList {
    #[serde(default)]
    chapter: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    chapter: String,
}

#[derive(Deserialize)]
struct ChapterPageData {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "imageSrc", default)]
    image_src: Vec<String>,
    #[serde(rename = "storageInter2", default)]
    storage_inter2: Option<bool>,
}

#[derive(Deserialize)]
struct ChapterPageImages {
    #[serde(rename = "imageSrc", default)]
    image_src: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"gambar":"/sample-cover.jpg","title":"Sample Softkomik","title_slug":"sample-softkomik"}],"maxPage":1,"page":1}"#;
const DETAILS_FIXTURE: &str = r#"{"gambar":"/sample-cover.jpg","title":"Sample Softkomik","author":"Sample Author","Genre":["Action"],"sinopsis":"Sample description.","status":"ongoing","type":"manga"}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapter":[{"chapter":"1"},{"chapter":"2"}]}"#;
const PAGES_FIXTURE: &str = r#"{"_id":"sample-page-data","imageSrc":["/sample-page-1.jpg"],"storageInter2":false}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_details_chapters_and_pages() {
        let list = parse_library(LIST_FIXTURE);
        assert_eq!(list.entries[0].key, "/sample-softkomik");
        let details = details_by_key("/sample-softkomik");
        assert_eq!(details.title, "Sample Softkomik");
        let chapters = parse_chapters(CHAPTERS_FIXTURE, "/sample-softkomik", false);
        assert_eq!(chapters.len(), 2);
        let data = extract_next::<ChapterPageData>(PAGES_FIXTURE, "_id").unwrap();
        assert_eq!(data.image_src[0], "/sample-page-1.jpg");
    }
}
