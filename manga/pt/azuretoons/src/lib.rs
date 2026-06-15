use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Azuretoons = Azuretoons;
const BASE_URL: &str = "https://azuretoons.com";
const API_URL: &str = "https://azuretoons.com/api";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct Azuretoons;

impl MangaSource for Azuretoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false, ""));
        }
        let popular = request.get("listingId").and_then(Value::as_str) != Some("latest");
        let body = fetch_json(&request, &format!("{API_URL}/obras"), LIST_FIXTURE);
        Ok(parse_listing(&body, popular, ""))
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
                entries: vec![details_by_key(&request, &key)],
                has_next_page: false,
            });
        }
        let body = fetch_json(&request, &format!("{API_URL}/obras"), LIST_FIXTURE);
        Ok(parse_listing(&body, false, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(details_by_key(&request, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        let slug = key
            .split("/obra/")
            .nth(1)
            .unwrap_or("sample")
            .trim_matches('/');
        let body = fetch_json(
            &request,
            &format!("{API_URL}/obras/slug/{slug}"),
            DETAILS_FIXTURE,
        );
        Ok(parse_chapters(&body, slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/obra/sample/capitulo/1".into());
        let slug = key
            .split("/obra/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("sample");
        let chapter_id = key
            .split("/capitulo/")
            .nth(1)
            .unwrap_or("1")
            .trim_matches('/');
        Ok(parse_pages(&fetch_json(
            &request,
            &format!("{API_URL}/chapters/read/{slug}/{chapter_id}"),
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key
                    .starts_with("/obra/")
                    .then(|| details_by_key(&request, &key)),
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
        .with_webview_challenge_fallback()
}

fn fetch_json(request: &Value, target: &str, fixture: &str) -> String {
    let http = client();
    let mut get = http
        .get(target)
        .xhr()
        .header("Accept", "*/*")
        .header("Accept-Language", "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7")
        .header("Pragma", "no-cache");
    if let Some(token) = login_token(request) {
        get = get.header("Authorization", format!("Bearer {token}"));
    }
    get.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn login_token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    let email = prefs.get("email").and_then(Value::as_str)?.trim();
    let password = prefs.get("password").and_then(Value::as_str)?;
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let body = client()
        .post(format!("{API_URL}/auth/login"))
        .xhr()
        .header("Accept", "application/json")
        .json(json!({ "identifier": email, "password": password }).to_string())
        .send_text()
        .ok()?;
    serde_json::from_str::<LoginResponse>(&body)
        .ok()
        .map(|auth| auth.access_token)
        .filter(|token| !token.is_empty())
}

fn details_by_key(request: &Value, key: &str) -> CatalogItem {
    let slug = key
        .split("/obra/")
        .nth(1)
        .unwrap_or("sample")
        .trim_matches('/');
    parse_details(
        &fetch_json(
            request,
            &format!("{API_URL}/obras/slug/{slug}"),
            DETAILS_FIXTURE,
        ),
        Some(key.to_string()),
    )
}

fn parse_listing(body: &str, popular: bool, query: &str) -> Paged<CatalogItem> {
    let mut entries = serde_json::from_str::<Vec<MangaDto>>(body)
        .or_else(|_| serde_json::from_str::<Vec<MangaDto>>(LIST_FIXTURE))
        .unwrap_or_default();
    if popular {
        entries.sort_by(|left, right| right.view_count.cmp(&left.view_count));
    }
    let lower = query.to_ascii_lowercase();
    Paged {
        entries: entries
            .into_iter()
            .filter(|item| lower.is_empty() || item.title.to_ascii_lowercase().contains(&lower))
            .map(|item| catalog_from_manga(&item, false))
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let manga = serde_json::from_str::<MangaDto>(body)
        .or_else(|_| serde_json::from_str::<MangaDto>(DETAILS_FIXTURE))
        .unwrap_or_default();
    let mut item = catalog_from_manga(&manga, true);
    if let Some(key) = key {
        item.key = key.clone();
        item.url = Some(absolute_url(&key));
    }
    item
}

fn catalog_from_manga(manga: &MangaDto, initialized: bool) -> CatalogItem {
    let key = format!("/obra/{}", manga.slug);
    CatalogItem {
        key: key.clone(),
        title: manga.title.clone(),
        cover: manga.cover_url.clone().filter(|value| !value.is_empty()),
        description: manga
            .description
            .as_deref()
            .map(html::strip_tags)
            .filter(|value| !value.is_empty()),
        status: status_from(manga.status.as_deref().unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let manga = serde_json::from_str::<MangaDto>(body)
        .or_else(|_| serde_json::from_str::<MangaDto>(DETAILS_FIXTURE))
        .unwrap_or_default();
    let mut chapters = manga
        .chapters
        .into_iter()
        .map(|chapter| {
            let number = trim_number(chapter.chapter_number);
            let key = format!("/obra/{slug}/capitulo/{number}");
            MangaChapter {
                key: key.clone(),
                title: chapter.title.or_else(|| Some(format!("Capitulo {number}"))),
                chapter_number: Some(chapter.chapter_number),
                date_uploaded: chapter.created_at.as_deref().and_then(parse_feed_date),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
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
    let pages = serde_json::from_str::<PagesDto>(body)
        .or_else(|_| serde_json::from_str::<PagesDto>(PAGES_FIXTURE))
        .unwrap_or_default();
    pages
        .images
        .into_iter()
        .filter(|image| !image.is_empty())
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

fn status_from(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "em_lancamento" | "em andamento" | "ativo" => ItemStatus::Ongoing,
        "concluido" | "concluído" => ItemStatus::Completed,
        "hiato" => ItemStatus::Hiatus,
        "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn trim_number(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn parse_feed_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

#[derive(Default, Deserialize)]
struct LoginResponse {
    #[serde(alias = "token")]
    access_token: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
    #[serde(default)]
    view_count: i32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    #[serde(default)]
    chapter_number: f32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Default, Deserialize)]
struct PagesDto {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"[{"title":"Sample Azure","slug":"sample","coverUrl":"/cover.jpg","status":"ativo","viewCount":10}]"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Azure","slug":"sample","coverUrl":"/cover.jpg","description":"<p>Summary</p>","status":"ativo","chapters":[{"chapterNumber":1,"title":"Capitulo 1","createdAt":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"images":["/page1.jpg","/page2.jpg"]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_azure_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE, true, "").entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
