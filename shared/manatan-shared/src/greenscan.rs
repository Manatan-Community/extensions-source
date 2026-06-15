use crate::{
    dates, html,
    manga::{self, image_headers},
    sdk::{
        CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, SearchRequest, UrlResolveResult,
        abi::{ExtensionError, ExtensionResult},
        http,
        source::MangaSource,
    },
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::marker::PhantomData;

pub trait GreenScanConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const API_URL: &'static str;
    const CDN_URL: &'static str;
    const CDN_API_URL: &'static str;
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const SCAN_ID: &'static str;
    const DEFAULT_GENRE_ID: &'static str = "1";
    const PAGE_SIZE: u64 = 26;
}

pub struct GreenScanSource<C>(PhantomData<C>);

impl<C> GreenScanSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: GreenScanConfig> MangaSource for GreenScanSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "popular" {
            format!(
                "{}/obras/ranking?tipo=visualizacoes_geral&limite={}&pagina={}&gen_id={}",
                C::API_URL,
                C::PAGE_SIZE,
                page(&request),
                C::DEFAULT_GENRE_ID
            )
        } else {
            format!(
                "{}/obras/atualizacoes?pagina={}&limite={}&gen_id={}",
                C::API_URL,
                page(&request),
                C::PAGE_SIZE,
                C::DEFAULT_GENRE_ID
            )
        };
        parse_list::<C>(&target, preferences(&request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url::<C>(&query) {
            return Ok(Paged {
                entries: vec![fetch_details::<C>(&key, preferences(&request))?],
                has_next_page: false,
            });
        }

        let mut params = vec![
            ("limite", C::PAGE_SIZE.to_string()),
            ("pagina", page(&request).to_string()),
        ];
        if !query.is_empty() {
            params.push(("obr_nome", query));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        if let Some(genre) = filter(filters, "genreId") {
            if genre.is_empty() {
                params.push(("todos_generos", "1".to_string()));
            } else {
                params.push(("gen_id", genre.to_string()));
            }
        }
        for (name, key) in [
            ("formt_id", "formatId"),
            ("stt_id", "statusId"),
            ("orderBy", "sort"),
            ("tag_ids", "tagIds"),
        ] {
            if let Some(value) = filter(filters, key) {
                params.push((name, value.to_string()));
            }
        }
        parse_list::<C>(
            &format!("{}/obras/search?{}", C::API_URL, query_string(&params)),
            preferences(&request),
        )
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        fetch_details::<C>(&key, preferences(&request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        let manga = get_api::<C, GreenScanMangaDto>(
            &format!("{}/obras/{}", C::API_URL, id_from_key(&key)),
            preferences(&request),
        )?;
        let mut chapters = manga
            .chapters
            .into_iter()
            .map(GreenScanChapterSimpleDto::into_chapter)
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
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/capitulo/1".to_string());
        let chapter = get_api::<C, GreenScanChapterDetailDto>(
            &format!("{}/capitulos/{}", C::API_URL, id_from_key(&key)),
            preferences(&request),
        )?;
        let pages = chapter.into_pages::<C>();
        if pages.is_empty() {
            return Err(extension_error("No GreenScan pages found"));
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
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(C::BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(C::BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url::<C>(input) {
            let is_chapter = key.contains("/capitulo/");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter)
                    .then(|| fetch_details::<C>(&key, preferences(&request)))
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

fn parse_list<C: GreenScanConfig>(
    target: &str,
    preferences: &Value,
) -> ExtensionResult<Paged<CatalogItem>> {
    let response = get_api::<C, GreenScanListDto<Vec<GreenScanMangaDto>>>(target, preferences)?;
    let has_next_page = response.has_next_page();
    Ok(Paged {
        entries: response
            .items
            .into_iter()
            .map(|item| item.into_item::<C>(false))
            .collect(),
        has_next_page,
    })
}

fn fetch_details<C: GreenScanConfig>(
    key: &str,
    preferences: &Value,
) -> ExtensionResult<CatalogItem> {
    let item = get_api::<C, GreenScanMangaDto>(
        &format!("{}/obras/{}", C::API_URL, id_from_key(key)),
        preferences,
    )?;
    Ok(item.into_item::<C>(true))
}

fn get_api<C: GreenScanConfig, T: for<'de> Deserialize<'de>>(
    target: &str,
    preferences: &Value,
) -> ExtensionResult<T> {
    let response = client::<C>(preferences).get(target).xhr().send_text()?;
    serde_json::from_str(&response).map_err(extension_error)
}

fn client<C: GreenScanConfig>(preferences: &Value) -> http::HttpClient {
    let mut client = http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::BASE_URL.trim_end_matches('/')))
        .with_header("Origin", C::BASE_URL)
        .with_header("scan-id", C::SCAN_ID)
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback();
    if let Some(token) = auth_token::<C>(preferences) {
        client = client.with_header("Authorization", format!("Bearer {token}"));
    }
    client
}

fn auth_token<C: GreenScanConfig>(preferences: &Value) -> Option<String> {
    if let Some(token) =
        preference(preferences, "bearerToken").or_else(|| preference(preferences, "token"))
    {
        return Some(token.to_string());
    }
    let email = preference(preferences, "email")?;
    let password = preference(preferences, "password")?;
    let body = json!({
        "login": email,
        "senha": password,
        "tipo_usuario": "usuario"
    });
    let response = http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::BASE_URL.trim_end_matches('/')))
        .with_header("Origin", C::BASE_URL)
        .with_header("scan-id", C::SCAN_ID)
        .post(format!("{}/auth/login", C::API_URL.trim_end_matches('/')))
        .json(body.to_string())
        .send_text()
        .ok()?;
    let auth = serde_json::from_str::<GreenScanLoginResponseDto>(&response).ok()?;
    auth.token.filter(|token| !token.trim().is_empty())
}

#[derive(Deserialize)]
struct GreenScanLoginResponseDto {
    #[serde(alias = "access_token")]
    token: Option<String>,
}

#[derive(Deserialize)]
struct GreenScanListDto<T> {
    #[serde(alias = "obras")]
    items: T,
    #[serde(
        default,
        alias = "currentPage",
        alias = "pagina_atual",
        alias = "pagina"
    )]
    current_page: u64,
    #[serde(
        default,
        alias = "totalPages",
        alias = "paginas",
        alias = "totalPaginas"
    )]
    total_pages: u64,
}

impl<T> GreenScanListDto<T> {
    fn has_next_page(&self) -> bool {
        self.total_pages > self.current_page
    }
}

#[derive(Deserialize)]
struct GreenScanTagDto {
    #[serde(default, alias = "tag_nome", alias = "name")]
    name: String,
}

#[derive(Deserialize)]
struct GreenScanStatusDto {
    #[serde(alias = "stt_nome", alias = "name")]
    name: String,
}

#[derive(Deserialize)]
struct GreenScanMangaDto {
    #[serde(alias = "obr_id")]
    id: i64,
    #[serde(alias = "obr_nome")]
    name: String,
    #[serde(default, alias = "obr_descricao")]
    description: Option<String>,
    #[serde(default, alias = "obr_imagem")]
    image: Option<String>,
    #[serde(default)]
    tags: Vec<GreenScanTagDto>,
    #[serde(default)]
    status: Option<GreenScanStatusDto>,
    #[serde(default, alias = "scan_id")]
    scan_id: i64,
    #[serde(default, alias = "capitulos")]
    chapters: Vec<GreenScanChapterSimpleDto>,
}

impl GreenScanMangaDto {
    fn into_item<C: GreenScanConfig>(self, initialized: bool) -> CatalogItem {
        let scan_id = if self.scan_id > 0 {
            self.scan_id.to_string()
        } else {
            C::SCAN_ID.to_string()
        };
        CatalogItem {
            key: format!("/obra/{}", self.id),
            title: self.name,
            cover: self.image.as_deref().map(|image| {
                image_url(
                    C::CDN_API_URL,
                    &scan_id,
                    self.id,
                    "",
                    image,
                    Some(300),
                    None,
                )
            }),
            description: self
                .description
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.trim().is_empty()),
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            status: parse_status(self.status.as_ref().map(|status| status.name.as_str())),
            url: Some(format!(
                "{}/obra/{}",
                C::BASE_URL.trim_end_matches('/'),
                self.id
            )),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct GreenScanChapterSimpleDto {
    #[serde(alias = "cap_id")]
    id: i64,
    #[serde(alias = "cap_nome")]
    name: String,
    #[serde(default, alias = "cap_numero")]
    number: Option<f32>,
    #[serde(default, alias = "cap_criado_em")]
    created_at: Option<String>,
}

impl GreenScanChapterSimpleDto {
    fn into_chapter(self) -> MangaChapter {
        MangaChapter {
            key: format!("/capitulo/{}", self.id),
            title: Some(self.name),
            chapter_number: self.number,
            date_uploaded: self
                .created_at
                .as_deref()
                .and_then(|value| value.split('T').next())
                .and_then(dates::parse_ymd),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct GreenScanPageDto {
    src: String,
    #[serde(default)]
    mime: Option<String>,
}

#[derive(Deserialize)]
struct GreenScanChapterDetailDto {
    #[serde(default, alias = "cap_numero")]
    number: Option<f32>,
    #[serde(default, alias = "cap_paginas")]
    pages: Vec<GreenScanPageDto>,
    #[serde(default, alias = "obra")]
    manga: Option<GreenScanMangaDto>,
}

impl GreenScanChapterDetailDto {
    fn into_pages<C: GreenScanConfig>(self) -> Vec<MangaPage> {
        let manga_id = self
            .manga
            .as_ref()
            .map(|manga| manga.id)
            .unwrap_or_default();
        let scan_id = self
            .manga
            .as_ref()
            .and_then(|manga| (manga.scan_id > 0).then_some(manga.scan_id.to_string()))
            .unwrap_or_else(|| C::SCAN_ID.to_string());
        let chapter_number = self
            .number
            .map(|number| number.to_string().trim_end_matches(".0").to_string())
            .unwrap_or_else(|| "0".to_string());
        self.pages
            .into_iter()
            .filter(|page| !page.src.trim().is_empty())
            .map(|page| MangaPage {
                content: PageContent::Url {
                    url: image_url(
                        C::CDN_URL,
                        &scan_id,
                        manga_id,
                        &chapter_number,
                        &page.src,
                        None,
                        page.mime.as_deref(),
                    ),
                    context: Some(image_headers(C::BASE_URL)),
                },
                ..MangaPage::default()
            })
            .collect()
    }
}

fn image_url(
    base: &str,
    scan_id: &str,
    manga_id: i64,
    chapter_number: &str,
    src: &str,
    width: Option<u64>,
    mime: Option<&str>,
) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    let trimmed = src.trim_start_matches('/');
    let path = if wp_like_path(trimmed) || mime.is_some() {
        if trimmed.starts_with("manga_") {
            format!("wp-content/uploads/WP-manga/data/{trimmed}")
        } else if trimmed.starts_with("WP-manga") || trimmed.starts_with("uploads/") {
            format!("wp-content/{trimmed}")
        } else if trimmed.starts_with("wp-content/") {
            trimmed.to_string()
        } else {
            format!("wp-content/uploads/WP-manga/data/{trimmed}")
        }
    } else if chapter_number.is_empty() {
        format!("scans/{scan_id}/obras/{manga_id}/{trimmed}")
    } else {
        format!("scans/{scan_id}/obras/{manga_id}/capitulos/{chapter_number}/{trimmed}")
    };
    let mut result = url::join_url(base, &normalize_slashes(&path));
    if let Some(width) = width {
        result.push_str(&format!("?width={width}"));
    }
    result
}

fn wp_like_path(src: &str) -> bool {
    src.starts_with("uploads/")
        || src.starts_with("wp-content/")
        || src.starts_with("manga_")
        || src.starts_with("WP-manga")
}

fn normalize_slashes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut previous = '\0';
    for ch in value.chars() {
        if ch == '/' && previous == '/' {
            continue;
        }
        out.push(ch);
        previous = ch;
    }
    out
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    let lower = value.unwrap_or_default().to_lowercase();
    if lower.contains("conclu") || lower.contains("completo") {
        ItemStatus::Completed
    } else if lower.contains("andamento") || lower.contains("ativo") {
        ItemStatus::Ongoing
    } else if lower.contains("hiato") {
        ItemStatus::Hiatus
    } else if lower.contains("cancel") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
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

fn preferences(request: &Value) -> &Value {
    request.get("preferences").unwrap_or(&Value::Null)
}

fn preference<'a>(preferences: &'a Value, key: &str) -> Option<&'a str> {
    preferences
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
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

fn key_from_url<C: GreenScanConfig>(input: &str) -> Option<String> {
    if !input.starts_with(C::BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(C::BASE_URL).trim();
    if path.starts_with("/obra/") || path.starts_with("/capitulo/") {
        Some(path.to_string())
    } else {
        None
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

    struct TestConfig;

    impl GreenScanConfig for TestConfig {
        const NAME: &'static str = "Test";
        const BASE_URL: &'static str = "https://example.org";
        const API_URL: &'static str = "https://api.example.org";
        const CDN_URL: &'static str = "https://cdn.example.org";
        const CDN_API_URL: &'static str = "https://api.example.org/cdn";
        const SCAN_ID: &'static str = "3";
    }

    #[test]
    fn builds_cdn_urls() {
        assert_eq!(
            image_url::<TestConfig>("3", 12, "", "cover.jpg", Some(300), None),
            "https://api.example.org/cdn/scans/3/obras/12/cover.jpg?width=300"
        );
        assert_eq!(
            image_url::<TestConfig>("3", 12, "4", "page.jpg", None, None),
            "https://cdn.example.org/scans/3/obras/12/capitulos/4/page.jpg"
        );
    }

    fn image_url<C: GreenScanConfig>(
        scan_id: &str,
        manga_id: i64,
        chapter_number: &str,
        src: &str,
        width: Option<u64>,
        mime: Option<&str>,
    ) -> String {
        super::image_url(
            if chapter_number.is_empty() {
                C::CDN_API_URL
            } else {
                C::CDN_URL
            },
            scan_id,
            manga_id,
            chapter_number,
            src,
            width,
            mime,
        )
    }
}
