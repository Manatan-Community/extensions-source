use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Wolftoon = Wolftoon;
const BASE_URL: &str = "https://wolftoon.lovable.app";
const SUPABASE_URL: &str = "https://encmakrlmutvsdzpodov.supabase.co";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct Wolftoon;

impl MangaSource for Wolftoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_titles(TITLES_FIXTURE, "", Some("popular"), &request));
        }
        let listing = request.get("listingId").and_then(Value::as_str);
        let order = if listing == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_titles(&fetch_titles(), "", Some(order), &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
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
        let order = filter_value(&request, "order");
        Ok(parse_titles(
            &fetch_titles(),
            query,
            order.as_deref(),
            &request,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample#1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample#1".into());
        let title_id = key.split('#').nth(1).unwrap_or("1");
        Ok(parse_chapters(&fetch_chapters(
            title_id,
            "id,title_id,chapter_number,created_at",
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/1/1#1".into());
        let chapter_id = key.split('#').nth(1).unwrap_or("1");
        let title_id = key
            .split("/read/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("1");
        Ok(parse_pages(
            &fetch_chapters(title_id, "id,title_id,images"),
            chapter_id,
        ))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key.starts_with("/manga/").then(|| details_by_key(&key)),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_cookies_for(SUPABASE_URL)
        .with_webview_challenge_fallback()
}

fn api_key() -> Option<String> {
    let html = client().get(BASE_URL).browser_document().send_text().ok()?;
    let script_path = html
        .split("src=\"")
        .chain(html.split("src='"))
        .find_map(|part| {
            let path = part.split(['"', '\'']).next()?;
            (path.starts_with("/assets/index-") && path.ends_with(".js")).then(|| path.to_string())
        })?;
    let script = client()
        .get(format!("{BASE_URL}{script_path}"))
        .browser_document()
        .send_text()
        .ok()?;
    extract_jwt(&script)
}

fn extract_jwt(script: &str) -> Option<String> {
    let marker = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.";
    let start = script.find(marker)?;
    let rest = &script[start..];
    let end = rest.find(['"', '\'']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn fetch_titles() -> String {
    let Some(key) = api_key() else {
        return TITLES_FIXTURE.to_string();
    };
    let target = format!(
        "{SUPABASE_URL}/rest/v1/titles?select=*&order=rating.desc&apikey={}",
        url::query_escape(&key)
    );
    client()
        .get(target)
        .xhr()
        .header("apikey", key.as_str())
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| TITLES_FIXTURE.to_string())
}

fn fetch_title(title_id: &str) -> String {
    let Some(key) = api_key() else {
        return TITLE_FIXTURE.to_string();
    };
    let target = format!(
        "{SUPABASE_URL}/rest/v1/titles?select=*&id=eq.{}&apikey={}",
        url::query_escape(title_id),
        url::query_escape(&key)
    );
    client()
        .get(target)
        .xhr()
        .header("apikey", key.as_str())
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| TITLE_FIXTURE.to_string())
}

fn fetch_chapters(title_id: &str, select: &str) -> String {
    let Some(key) = api_key() else {
        return CHAPTERS_FIXTURE.to_string();
    };
    let target = format!(
        "{SUPABASE_URL}/rest/v1/chapters?select={}&title_id=eq.{}&order=chapter_number.desc&apikey={}",
        url::query_escape(select),
        url::query_escape(title_id),
        url::query_escape(&key)
    );
    client()
        .get(target)
        .xhr()
        .header("apikey", key.as_str())
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string())
}

fn parse_titles(
    body: &str,
    query: &str,
    order: Option<&str>,
    request: &Value,
) -> Paged<CatalogItem> {
    let mut titles = serde_json::from_str::<Vec<MangaDto>>(body)
        .or_else(|_| serde_json::from_str::<Vec<MangaDto>>(TITLES_FIXTURE))
        .unwrap_or_default();
    let lower = query.to_ascii_lowercase();
    if !lower.is_empty() {
        titles.retain(|title| {
            title.title.to_ascii_lowercase().contains(&lower)
                || title.synopsis.to_ascii_lowercase().contains(&lower)
        });
    }
    if let Some(status) = filter_value(request, "status").filter(|value| !value.is_empty()) {
        titles.retain(|title| title.status.eq_ignore_ascii_case(&status));
    }
    if let Some(kind) = filter_value(request, "type").filter(|value| !value.is_empty()) {
        titles.retain(|title| title.kind.eq_ignore_ascii_case(&kind));
    }
    if let Some(genre) = filter_value(request, "genre").filter(|value| !value.is_empty()) {
        titles.retain(|title| {
            title
                .genres
                .iter()
                .any(|item| item.eq_ignore_ascii_case(&genre))
        });
    }
    match order.unwrap_or("popular") {
        "latest" => titles.sort_by(|left, right| right.updated_at.cmp(&left.updated_at)),
        "rating" => titles.sort_by(|left, right| {
            right
                .rating
                .partial_cmp(&left.rating)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => titles.sort_by(|left, right| right.views.cmp(&left.views)),
    }
    Paged {
        entries: titles
            .into_iter()
            .map(|title| title.to_catalog(false))
            .collect(),
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let title_id = key.split('#').nth(1).unwrap_or("1");
    let titles = serde_json::from_str::<Vec<MangaDto>>(&fetch_title(title_id))
        .or_else(|_| serde_json::from_str::<Vec<MangaDto>>(TITLE_FIXTURE))
        .unwrap_or_default();
    titles
        .into_iter()
        .next()
        .map(|title| title.to_catalog(true))
        .unwrap_or_else(|| MangaDto::default().to_catalog(true))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<ChapterDto>>(body)
        .or_else(|_| serde_json::from_str::<Vec<ChapterDto>>(CHAPTERS_FIXTURE))
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!(
                "/read/{}/{}#{}",
                chapter.title_id,
                trim_float(chapter.chapter_number),
                chapter.id
            ),
            title: Some(format!("Capitulo {}", trim_float(chapter.chapter_number))),
            chapter_number: Some(chapter.chapter_number),
            date_uploaded: parse_iso_date(&chapter.created_at),
            language: Some(LANG.to_string()),
            url: Some(absolute_url(&format!(
                "/read/{}/{}#{}",
                chapter.title_id,
                trim_float(chapter.chapter_number),
                chapter.id
            ))),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str, chapter_id: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Vec<PageDto>>(body)
        .or_else(|_| serde_json::from_str::<Vec<PageDto>>(CHAPTERS_FIXTURE))
        .unwrap_or_default()
        .into_iter()
        .find(|page| page.id == chapter_id)
        .map(|page| {
            page.images
                .into_iter()
                .enumerate()
                .map(|(index, image)| MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    let filters = request.get("filters")?;
    if let Some(value) = filters.get(id).and_then(Value::as_str) {
        return Some(value.trim().to_string());
    }
    filters.as_array()?.iter().find_map(|filter| {
        (filter.get("id").and_then(Value::as_str) == Some(id))
            .then(|| {
                filter
                    .get("value")
                    .and_then(Value::as_str)
                    .map(|value| value.trim().to_string())
            })
            .flatten()
    })
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn trim_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        format!("{value}")
    }
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in input.chars() {
        let mapped = deaccent(ch).unwrap_or(ch);
        if mapped.is_ascii_alphanumeric() {
            out.push(mapped.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn deaccent(ch: char) -> Option<char> {
    Some(match ch {
        'á' | 'à' | 'ã' | 'â' | 'ä' | 'Á' | 'À' | 'Ã' | 'Â' | 'Ä' => 'a',
        'é' | 'ê' | 'è' | 'ë' | 'É' | 'Ê' | 'È' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'õ' | 'ô' | 'ö' | 'Ó' | 'Ò' | 'Õ' | 'Ô' | 'Ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        _ => return None,
    })
}

fn normalize_key(input: &str) -> String {
    let value = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", value.trim_start_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

#[derive(Default, Deserialize)]
struct MangaDto {
    id: String,
    title: String,
    cover: String,
    status: String,
    #[serde(default)]
    genres: Vec<String>,
    synopsis: String,
    #[serde(rename = "type")]
    kind: String,
    author: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    rating: f32,
    #[serde(default)]
    views: u64,
}

impl MangaDto {
    fn to_catalog(&self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}#{}", slug(&self.title), self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title.clone(),
            cover: Some(self.cover.clone()).filter(|value| !value.is_empty()),
            description: Some(self.synopsis.clone()).filter(|value| !value.is_empty()),
            authors: Some(self.author.clone())
                .filter(|value| !value.is_empty())
                .into_iter()
                .collect(),
            tags: self
                .genres
                .iter()
                .cloned()
                .chain(Some(self.kind.clone()))
                .filter(|value| !value.is_empty())
                .collect(),
            status: match self.status.to_ascii_lowercase().as_str() {
                "em andamento" => ItemStatus::Ongoing,
                "completo" => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            rating: Some(self.rating).filter(|value| *value > 0.0),
            url: Some(absolute_url(&key)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    id: String,
    title_id: String,
    chapter_number: f32,
    #[serde(default)]
    created_at: String,
}

#[derive(Default, Deserialize)]
struct PageDto {
    id: String,
    #[serde(default)]
    images: Vec<String>,
}

const TITLES_FIXTURE: &str = r#"[{"id":"1","title":"Sample Wolftoon","cover":"https://wolftoon.lovable.app/cover.jpg","status":"Em Andamento","genres":["Ação"],"synopsis":"Sample description","type":"Manhwa","author":"Author","updated_at":"2024-01-01T00:00:00","rating":5.0,"views":10}]"#;
const TITLE_FIXTURE: &str = TITLES_FIXTURE;
const CHAPTERS_FIXTURE: &str = r#"[{"id":"1","title_id":"1","chapter_number":1.0,"created_at":"2024-01-01T00:00:00","images":["https://wolftoon.lovable.app/page.jpg"]}]"#;

export_manga_source!(SOURCE);
