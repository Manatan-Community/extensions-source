use manatan_extension::export_manga_source;
use manatan_shared::{
    dates, html,
    manga::{self, image_headers},
    sdk::{
        CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, SearchRequest, UrlResolveResult,
        abi::{ExtensionError, ExtensionResult},
        http::HttpClient,
        source::MangaSource,
    },
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: KuroMangas = KuroMangas;
const BASE_URL: &str = "https://beta.kuromangas.com";
const API_URL: &str = "https://beta.kuromangas.com/api";
const CDN_URL: &str = "https://cdn.kuromangas.com";

struct KuroMangas;

impl MangaSource for KuroMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "popular" {
            format!(
                "{API_URL}/mangas?page={}&limit=24&sort=view_count&order=DESC",
                page(&request)
            )
        } else {
            format!(
                "{API_URL}/chapters/recent?page={}&limit=24&days=30",
                page(&request)
            )
        };
        if listing(&request) == "popular" {
            let response: MangaListResponse = get_json(&target, &request)?;
            Ok(Paged {
                entries: response.data.into_iter().map(MangaDto::into_item).collect(),
                has_next_page: response.pagination.has_next_page(),
            })
        } else {
            let response: LatestResponse = get_json(&target, &request)?;
            Ok(Paged {
                entries: response
                    .data
                    .into_iter()
                    .map(LatestMangaDto::into_item)
                    .collect(),
                has_next_page: response.pagination.has_next_page(),
            })
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key, &request)?],
                has_next_page: false,
            });
        }
        let sort = filter(&request, "sort").unwrap_or("created_at");
        let order = filter(&request, "order").unwrap_or_else(|| sort_order(sort));
        let mut params = vec![
            ("page", page(&request).to_string()),
            ("limit", "24".to_string()),
            ("sort", sort.to_string()),
            ("order", order.to_string()),
        ];
        if !query.is_empty() {
            params.push(("search", query));
        }
        let response: MangaListResponse = get_json(
            &format!("{API_URL}/mangas?{}", query_string(&params)),
            &request,
        )?;
        Ok(Paged {
            entries: response.data.into_iter().map(MangaDto::into_item).collect(),
            has_next_page: response.pagination.has_next_page(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".to_string());
        fetch_details(&key, &request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/1".to_string());
        let response: MangaDetailsResponse =
            get_json(&format!("{API_URL}/mangas/{}", id_from_key(&key)), &request)?;
        let mut chapters = response
            .chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter(response.manga.id))
            .collect::<Vec<_>>();
        chapters.sort_by(|a, b| {
            b.chapter_number
                .unwrap_or(-1.0)
                .total_cmp(&a.chapter_number.unwrap_or(-1.0))
        });
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/1/1".to_string());
        let response: ChapterPagesResponse = get_json(
            &format!("{API_URL}/chapters/{}", id_from_key(&key)),
            &request,
        )?;
        let pages = response
            .pages
            .into_iter()
            .map(|page| {
                let fixed = page.replacen("/uploads/", "/", 1);
                let image_url = if fixed.starts_with("http://") || fixed.starts_with("https://") {
                    fixed
                } else {
                    url::join_url(CDN_URL, &fixed)
                };
                MangaPage {
                    content: PageContent::Url {
                        url: image_url,
                        context: Some(image_headers(BASE_URL)),
                    },
                    ..MangaPage::default()
                }
            })
            .collect::<Vec<_>>();
        if pages.is_empty() {
            return Err(extension_error("No KuroMangas pages found"));
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let preferences = request.get("preferences").cloned().unwrap_or(Value::Null);
        let popular = self
            .list(json!({"page": 1, "listingId": "popular", "preferences": preferences.clone()}))?;
        let latest =
            self.list(json!({"page": 1, "listingId": "latest", "preferences": preferences}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", id_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key
                .trim_start_matches("/chapter/")
                .split('/')
                .collect::<Vec<_>>();
            format!(
                "{BASE_URL}/reader/{}/{}",
                parts.first().copied().unwrap_or_default(),
                parts.get(1).copied().unwrap_or_default()
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let is_chapter = key.starts_with("/chapter/");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter)
                    .then(|| fetch_details(&key, &request))
                    .transpose()?,
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

fn fetch_details(key: &str, request: &Value) -> ExtensionResult<CatalogItem> {
    let response: MangaDetailsResponse =
        get_json(&format!("{API_URL}/mangas/{}", id_from_key(key)), request)?;
    Ok(response.manga.into_item())
}

fn get_json<T: for<'de> Deserialize<'de>>(target: &str, request: &Value) -> ExtensionResult<T> {
    let token = auth_token(request)?;
    let text = client()
        .with_header("Authorization", format!("Bearer {token}"))
        .get(target)
        .xhr()
        .send_text()?;
    serde_json::from_str(&text).map_err(extension_error)
}

fn auth_token(request: &Value) -> ExtensionResult<String> {
    let preferences = request.get("preferences").unwrap_or(&Value::Null);
    if let Some(token) =
        preference(preferences, "bearerToken").or_else(|| preference(preferences, "token"))
    {
        return Ok(token.to_string());
    }
    let email = preference(preferences, "kuromangas_email")
        .or_else(|| preference(preferences, "email"))
        .ok_or_else(|| {
            extension_error("KuroMangas requires email/password or bearerToken preferences")
        })?;
    let password = preference(preferences, "kuromangas_password")
        .or_else(|| preference(preferences, "password"))
        .ok_or_else(|| {
            extension_error("KuroMangas requires email/password or bearerToken preferences")
        })?;
    let response = client()
        .post(format!("{API_URL}/auth/login"))
        .json(json!({ "email": email, "password": password }).to_string())
        .send_text()?;
    let login: LoginResponse = serde_json::from_str(&response).map_err(extension_error)?;
    Ok(login.token)
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Accept-Language", "pt-BR,pt;q=0.8,en-US;q=0.5,en;q=0.3")
        .with_referer(format!("{BASE_URL}/catalogo"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

#[derive(Deserialize)]
struct MangaListResponse {
    data: Vec<MangaDto>,
    pagination: PaginationDto,
}

#[derive(Deserialize)]
struct PaginationDto {
    page: u64,
    #[serde(default, alias = "total_pages")]
    total_pages: Option<u64>,
    #[serde(default, rename = "hasNext")]
    has_next: Option<bool>,
}

impl PaginationDto {
    fn has_next_page(&self) -> bool {
        self.has_next
            .unwrap_or_else(|| self.page < self.total_pages.unwrap_or(1))
    }
}

#[derive(Deserialize)]
struct MangaDto {
    id: i64,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "cover_image")]
    cover_image: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default, rename = "alternative_titles")]
    alternative_titles: Option<Vec<String>>,
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        let mut description = self
            .description
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        if let Some(titles) = self.alternative_titles.filter(|titles| !titles.is_empty()) {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Titulos alternativos: ");
            description.push_str(&titles.join(", "));
        }
        CatalogItem {
            key: format!("/manga/{}", self.id),
            title: self.title,
            cover: self.cover_image.map(|image| build_thumbnail_url(&image)),
            description: (!description.trim().is_empty()).then_some(description),
            authors: self.author.into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            tags: self.genres.unwrap_or_default(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/manga/{}", self.id)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaDetailsResponse {
    manga: MangaDto,
    chapters: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "chapter_number")]
    chapter_number: Option<String>,
    #[serde(default, rename = "upload_date")]
    upload_date: Option<String>,
}

impl ChapterDto {
    fn into_chapter(self, manga_id: i64) -> MangaChapter {
        let number = self.chapter_number.unwrap_or_default();
        let title = self.title.unwrap_or_default();
        let name = if number.is_empty() {
            if title.is_empty() {
                format!("Capitulo {}", self.id)
            } else {
                title
            }
        } else if title.is_empty() {
            format!("Capitulo {}", clean_number(&number))
        } else {
            format!("Capitulo {} - {}", clean_number(&number), title)
        };
        MangaChapter {
            key: format!("/chapter/{manga_id}/{}", self.id),
            title: Some(name),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: self
                .upload_date
                .as_deref()
                .and_then(|date| date.split('T').next())
                .and_then(dates::parse_ymd),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct LatestResponse {
    data: Vec<LatestMangaDto>,
    pagination: PaginationDto,
}

#[derive(Deserialize)]
struct LatestMangaDto {
    #[serde(rename = "manga_id")]
    manga_id: i64,
    #[serde(rename = "manga_title")]
    manga_title: String,
    #[serde(default, rename = "manga_cover")]
    manga_cover: Option<String>,
    #[serde(default, rename = "manga_genres")]
    manga_genres: Option<Vec<String>>,
}

impl LatestMangaDto {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/manga/{}", self.manga_id),
            title: self.manga_title,
            cover: self.manga_cover.map(|image| build_thumbnail_url(&image)),
            tags: self.manga_genres.unwrap_or_default(),
            url: Some(format!("{BASE_URL}/manga/{}", self.manga_id)),
            language: Some("pt-BR".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct ChapterPagesResponse {
    pages: Vec<String>,
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

fn build_thumbnail_url(path: &str) -> String {
    let clean = path.trim_start_matches('/').trim_start_matches("uploads/");
    url::join_url(CDN_URL, clean)
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_lowercase().as_str() {
        "ongoing" | "em andamento" => ItemStatus::Ongoing,
        "completed" | "completo" => ItemStatus::Completed,
        "hiatus" | "em hiato" => ItemStatus::Hiatus,
        "cancelled" | "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn preference<'a>(preferences: &'a Value, key: &str) -> Option<&'a str> {
    preferences
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn sort_order(sort: &str) -> &str {
    match sort {
        "created_at_asc" | "title" => "ASC",
        "title_desc" => "DESC",
        _ => "DESC",
    }
}

fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn id_from_key(key: &str) -> &str {
    key.trim_start_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("1")
        .split(['?', '#'])
        .next()
        .unwrap_or("1")
}

fn clean_number(value: &str) -> &str {
    value.trim_end_matches(".0")
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["manga", id] => Some(format!("/manga/{id}")),
        ["reader", manga_id, chapter_id] => Some(format!("/chapter/{manga_id}/{chapter_id}")),
        _ => None,
    }
}

fn extension_error(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_site_urls_to_keys() {
        assert_eq!(
            key_from_url("https://beta.kuromangas.com/manga/123"),
            Some("/manga/123".to_string())
        );
        assert_eq!(
            key_from_url("https://beta.kuromangas.com/reader/123/456"),
            Some("/chapter/123/456".to_string())
        );
    }
}

export_manga_source!(SOURCE);
