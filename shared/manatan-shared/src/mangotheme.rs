use crate::{
    dates,
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
use aes::Aes256;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::marker::PhantomData;

type Aes256CbcDec = cbc::Decryptor<Aes256>;

pub trait MangoThemeConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const API_URL: &'static str;
    const CDN_URL: &'static str;
    const LANG: &'static str;
    const CONTENT_RATING: &'static str = "suggestive";
    const ENCRYPTION_KEY: &'static str;
    const WEB_MANGA_PATH: &'static str = "obra";
    const WEB_CHAPTER_PATH: &'static str = "capitulo";
    const LATEST_PAGE_SIZE: u64 = 24;
    const SEARCH_PAGE_SIZE: u64 = 20;
    const REQUIRES_LOGIN: bool = false;

    fn extra_headers() -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

pub struct MangoThemeSource<C>(PhantomData<C>);

impl<C> MangoThemeSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: MangoThemeConfig> MangaSource for MangoThemeSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "popular" {
            format!("{}/obras/top10/views?periodo=total", C::API_URL)
        } else {
            format!(
                "{}/capitulos/recentes?pagina={}&limite={}",
                C::API_URL,
                page(&request),
                C::LATEST_PAGE_SIZE
            )
        };
        let preferences = preferences(&request);
        let response: MangoThemeResponse<Vec<MangoThemeMangaDto>> =
            get_api::<C, Vec<MangoThemeMangaDto>>(&target, preferences)?;
        Ok(Paged {
            entries: response
                .payload
                .unwrap_or_default()
                .into_iter()
                .map(MangoThemeMangaDto::into_item::<C>)
                .collect(),
            has_next_page: response
                .pagination
                .is_some_and(|pagination| pagination.has_next_page),
        })
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
            ("pagina", page(&request).to_string()),
            ("limite", C::SEARCH_PAGE_SIZE.to_string()),
        ];
        if !query.is_empty() {
            params.push(("busca", query));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        for (name, key) in [
            ("status_id", "statusId"),
            ("formato_id", "formatId"),
            ("min_capitulos", "minChapters"),
            ("tag_ids", "tagIds"),
        ] {
            if let Some(value) = filter(filters, key) {
                params.push((name, value.to_string()));
            }
        }
        let target = format!("{}/obras?{}", C::API_URL, query_string(&params));
        let response: MangoThemeResponse<Vec<MangoThemeMangaDto>> =
            get_api::<C, Vec<MangoThemeMangaDto>>(&target, preferences(&request))?;
        Ok(Paged {
            entries: response
                .payload
                .unwrap_or_default()
                .into_iter()
                .map(MangoThemeMangaDto::into_item::<C>)
                .collect(),
            has_next_page: response
                .pagination
                .is_some_and(|pagination| pagination.has_next_page),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        fetch_details::<C>(&key, preferences(&request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".to_string());
        let preferences = preferences(&request);
        let response: MangoThemeResponse<MangoThemeMangaDto> = get_api::<C, MangoThemeMangaDto>(
            &format!("{}/obras/{}", C::API_URL, manga_id(&key)),
            preferences,
        )?;
        let manga = response
            .payload
            .ok_or_else(|| extension_error("MangoTheme manga payload missing"))?;
        let slug = manga.slug.clone().or_else(|| stored_slug(&key));
        let mut chapters = manga
            .chapters
            .into_iter()
            .map(|chapter| chapter.into_chapter::<C>(slug.as_deref()))
            .collect::<Vec<_>>();
        chapters.sort_by(|a, b| {
            b.chapter_number
                .unwrap_or(-1.0)
                .total_cmp(&a.chapter_number.unwrap_or(-1.0))
        });
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/obra/1/capitulo/1".to_string());
        let preferences = preferences(&request);
        let response: MangoThemeResponse<MangoThemeChapterDto> = get_api::<C, MangoThemeChapterDto>(
            &format!(
                "{}/obras/{}/capitulos/{}",
                C::API_URL,
                chapter_manga_id(&key),
                chapter_number(&key)
            ),
            preferences,
        )?;
        let mut pages = response
            .payload
            .ok_or_else(|| extension_error("MangoTheme chapter payload missing"))?
            .pages;
        pages.sort_by_key(|page| page.number);
        let entries = pages
            .into_iter()
            .filter_map(|page| page.url)
            .filter(|value| !value.trim().is_empty())
            .map(|image| MangaPage {
                content: PageContent::Url {
                    url: absolute_cdn::<C>(&image),
                    context: Some(image_headers(C::BASE_URL)),
                },
                ..MangaPage::default()
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(extension_error("No valid MangoTheme page URLs found"));
        }
        Ok(entries)
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
        Ok(manga::request_key(&request, "manga").map(|key| manga_url::<C>(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url::<C>(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url::<C>(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details::<C>(&key, preferences(&request))?),
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

fn client<C: MangoThemeConfig>(preferences: &Value) -> ExtensionResult<http::HttpClient> {
    let mut client = http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::BASE_URL.trim_end_matches('/')))
        .with_header(
            "Accept-Language",
            format!("{}, en-US;q=0.8, en;q=0.7", C::LANG),
        )
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback();
    for (name, value) in C::extra_headers() {
        client = client.with_header(name, value);
    }
    if C::REQUIRES_LOGIN {
        let token = auth_token::<C>(preferences)?.ok_or_else(|| {
            extension_error(format!(
                "{} requires login. Set source preferences email/password or bearerToken.",
                C::NAME
            ))
        })?;
        client = client.with_header("Authorization", format!("Bearer {token}"));
    }
    Ok(client)
}

fn get_api<C: MangoThemeConfig, T: for<'de> Deserialize<'de>>(
    target: &str,
    preferences: &Value,
) -> ExtensionResult<MangoThemeResponse<T>> {
    let response = client::<C>(preferences)?.get(target).xhr().send()?;
    let encrypted = response.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("x-encrypted") && value.eq_ignore_ascii_case("true")
    });
    let mut text = response.text.unwrap_or_default();
    if encrypted || !text.trim_start().starts_with(['{', '[']) {
        text = decrypt_payload(&text, C::ENCRYPTION_KEY)?;
    }
    serde_json::from_str(&text).map_err(extension_error)
}

fn fetch_details<C: MangoThemeConfig>(
    key: &str,
    preferences: &Value,
) -> ExtensionResult<CatalogItem> {
    let response: MangoThemeResponse<MangoThemeMangaDto> = get_api::<C, _>(
        &format!("{}/obras/{}", C::API_URL, manga_id(key)),
        preferences,
    )?;
    response
        .payload
        .map(MangoThemeMangaDto::into_item::<C>)
        .ok_or_else(|| extension_error("MangoTheme details payload missing"))
}

fn auth_token<C: MangoThemeConfig>(preferences: &Value) -> ExtensionResult<Option<String>> {
    if let Some(token) = preference(preferences, "bearerToken")
        .or_else(|| preference(preferences, "token"))
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(token.to_string()));
    }
    let Some(email) = preference(preferences, "email") else {
        return Ok(None);
    };
    let Some(password) = preference(preferences, "password") else {
        return Ok(None);
    };
    if email.is_empty() || password.is_empty() {
        return Ok(None);
    }
    let response = http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::BASE_URL.trim_end_matches('/')))
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback()
        .post(format!("{}/auth/login", C::API_URL.trim_end_matches('/')))
        .json(json!({ "email": email, "senha": password }).to_string())
        .send_text()?;
    let login: MangoThemeLoginResponseDto =
        serde_json::from_str(&response).map_err(extension_error)?;
    Ok(login.token.filter(|token| !token.trim().is_empty()))
}

fn decrypt_payload(payload: &str, key: &str) -> ExtensionResult<String> {
    let trimmed = payload.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(trimmed.to_string());
    }
    let (iv_hex, cipher_hex) = trimmed
        .split_once(':')
        .ok_or_else(|| extension_error("Invalid MangoTheme encrypted payload"))?;
    let iv = hex_to_bytes(iv_hex)?;
    let cipher_text = hex_to_bytes(cipher_hex)?;
    let mut hasher = Sha256::new();
    hasher.update(format!("{key}salt").as_bytes());
    let key_bytes = hasher.finalize();
    let plain = Aes256CbcDec::new((&key_bytes[..]).into(), (&iv[..]).into())
        .decrypt_padded_vec_mut::<Pkcs7>(&cipher_text)
        .map_err(|error| extension_error(format!("{error:?}")))?;
    String::from_utf8(plain).map_err(extension_error)
}

fn hex_to_bytes(value: &str) -> ExtensionResult<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(extension_error("Invalid hex string length"));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(extension_error))
        .collect()
}

#[derive(Deserialize)]
struct MangoThemeResponse<T> {
    #[serde(
        alias = "dados",
        alias = "obras",
        alias = "obra",
        alias = "data",
        alias = "capitulos",
        alias = "capitulo"
    )]
    payload: Option<T>,
    #[serde(default)]
    pagination: Option<MangoThemePaginationDto>,
}

#[derive(Deserialize)]
struct MangoThemePaginationDto {
    #[serde(default, rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct MangoThemeLoginResponseDto {
    #[serde(default, alias = "sucesso")]
    _success: bool,
    #[serde(default, alias = "access_token")]
    token: Option<String>,
}

#[derive(Deserialize)]
struct MangoThemeMangaDto {
    id: Option<i64>,
    #[serde(alias = "nome")]
    title: String,
    #[serde(default, alias = "nome_url", alias = "permalink", alias = "url")]
    slug: Option<String>,
    #[serde(default, rename = "coverImage", alias = "imagem")]
    cover_image: Option<String>,
    #[serde(default, alias = "descricao")]
    description: Option<String>,
    #[serde(default, alias = "status_id")]
    status_id: Option<i64>,
    #[serde(default, alias = "status_nome")]
    status_name: Option<String>,
    #[serde(default, alias = "banner_imagem")]
    banner_image: Option<String>,
    #[serde(default)]
    tags: Vec<MangoThemeTagDto>,
    #[serde(default, alias = "capitulos")]
    chapters: Vec<MangoThemeChapterDto>,
}

impl MangoThemeMangaDto {
    fn into_item<C: MangoThemeConfig>(self) -> CatalogItem {
        let id = self.id.unwrap_or_default();
        let slug = self.slug.clone();
        CatalogItem {
            key: internal_manga_key(id, slug.as_deref()),
            title: self.title,
            cover: self
                .cover_image
                .or(self.banner_image)
                .map(|image| absolute_cdn::<C>(&image)),
            description: self.description.filter(|value| !value.trim().is_empty()),
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            status: parse_status(self.status_name.as_deref(), self.status_id),
            url: Some(manga_url::<C>(&internal_manga_key(id, slug.as_deref()))),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangoThemeTagDto {
    #[serde(alias = "nome")]
    name: String,
}

#[derive(Deserialize)]
struct MangoThemeChapterDto {
    #[serde(alias = "obra_id")]
    manga_id: i64,
    #[serde(alias = "numero")]
    number: Value,
    #[serde(default, alias = "nome")]
    title: Option<String>,
    #[serde(default, alias = "criado_em", alias = "atualizado_em")]
    created_at: Option<String>,
    #[serde(default, alias = "paginas")]
    pages: Vec<MangoThemePageDto>,
}

impl MangoThemeChapterDto {
    fn chapter_number_text(&self) -> String {
        match &self.number {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            _ => String::new(),
        }
        .trim_end_matches(".0")
        .to_string()
    }

    fn into_chapter<C: MangoThemeConfig>(self, slug: Option<&str>) -> MangaChapter {
        let number = self.chapter_number_text();
        let key = internal_chapter_key(self.manga_id, &number, slug);
        MangaChapter {
            key: key.clone(),
            title: Some(
                self.title
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("Capitulo {number}")),
            ),
            chapter_number: number.parse::<f32>().ok(),
            date_uploaded: self
                .created_at
                .as_deref()
                .and_then(|date| date.split('T').next())
                .and_then(dates::parse_ymd),
            url: Some(chapter_url::<C>(&key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct MangoThemePageDto {
    #[serde(default, alias = "numero")]
    number: i64,
    #[serde(
        default,
        alias = "cdn_id",
        alias = "imagem",
        alias = "image",
        alias = "src",
        alias = "link",
        alias = "path",
        alias = "arquivo"
    )]
    url: Option<String>,
}

fn parse_status(name: Option<&str>, id: Option<i64>) -> ItemStatus {
    match name.unwrap_or_default().trim() {
        "Ativo" | "Em Andamento" => ItemStatus::Ongoing,
        "Concluído" | "Concluido" => ItemStatus::Completed,
        "Hiato" | "Pausado" => ItemStatus::Hiatus,
        "Cancelado" => ItemStatus::Cancelled,
        _ => match id {
            Some(1 | 6) => ItemStatus::Ongoing,
            Some(2 | 5) => ItemStatus::Hiatus,
            Some(3) => ItemStatus::Completed,
            Some(4) => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
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

fn absolute_cdn<C: MangoThemeConfig>(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(C::CDN_URL, value)
    }
}

fn internal_manga_key(id: i64, slug: Option<&str>) -> String {
    let slug = slug.and_then(clean_slug);
    match slug {
        Some(slug) => format!("/obra/{id}?slug={slug}"),
        None => format!("/obra/{id}"),
    }
}

fn internal_chapter_key(manga_id: i64, number: &str, slug: Option<&str>) -> String {
    let slug = slug.and_then(clean_slug);
    match slug {
        Some(slug) => format!("/obra/{manga_id}/capitulo/{number}?slug={slug}"),
        None => format!("/obra/{manga_id}/capitulo/{number}"),
    }
}

fn clean_slug(value: &str) -> Option<&str> {
    value
        .trim()
        .trim_end_matches('/')
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .filter(|value| !value.is_empty())
}

fn manga_id(key: &str) -> &str {
    key.trim_start_matches("/obra/")
        .split(['/', '?', '#', '-'])
        .next()
        .unwrap_or(key)
}

fn chapter_manga_id(key: &str) -> &str {
    key.trim_start_matches("/obra/")
        .split('/')
        .next()
        .unwrap_or(key)
}

fn chapter_number(key: &str) -> &str {
    key.trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("1")
        .split(['?', '#'])
        .next()
        .unwrap_or("1")
}

fn stored_slug(key: &str) -> Option<String> {
    key.split("?slug=")
        .nth(1)
        .and_then(|value| value.split('&').next())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn manga_url<C: MangoThemeConfig>(key: &str) -> String {
    let reference = stored_slug(key).unwrap_or_else(|| manga_id(key).to_string());
    format!(
        "{}/{}/{}",
        C::BASE_URL.trim_end_matches('/'),
        C::WEB_MANGA_PATH.trim_matches('/'),
        reference
    )
}

fn chapter_url<C: MangoThemeConfig>(key: &str) -> String {
    let reference = stored_slug(key).unwrap_or_else(|| chapter_manga_id(key).to_string());
    format!(
        "{}/{}/{}/{}/{}",
        C::BASE_URL.trim_end_matches('/'),
        C::WEB_MANGA_PATH.trim_matches('/'),
        reference,
        C::WEB_CHAPTER_PATH.trim_matches('/'),
        chapter_number(key)
    )
}

fn key_from_url<C: MangoThemeConfig>(input: &str) -> Option<String> {
    if !input.starts_with(C::BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(C::BASE_URL);
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() >= 2 && parts[0] == C::WEB_MANGA_PATH.trim_matches('/') {
        let reference = parts[1];
        let id = reference.split('-').next().unwrap_or(reference);
        return Some(format!("/obra/{id}?slug={reference}"));
    }
    None
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

    impl MangoThemeConfig for TestConfig {
        const NAME: &'static str = "Test";
        const BASE_URL: &'static str = "https://example.org";
        const API_URL: &'static str = "https://api.example.org/api";
        const CDN_URL: &'static str = "https://cdn.example.org";
        const LANG: &'static str = "pt-BR";
        const ENCRYPTION_KEY: &'static str = "test-key";
    }

    #[test]
    fn decrypts_payloads() {
        let encrypted = "00112233445566778899aabbccddeeff:54e8d4cfd33e4ccaa7962d73e58e18aa";
        let decrypted = decrypt_payload(encrypted, "test-key");
        assert!(decrypted.is_err());
        assert_eq!(
            decrypt_payload("{\"ok\":true}", "test-key").unwrap(),
            "{\"ok\":true}"
        );
    }

    #[test]
    fn maps_manga_and_chapter_urls() {
        let key = internal_manga_key(42, Some("sample-title"));
        assert_eq!(
            manga_url::<TestConfig>(&key),
            "https://example.org/obra/sample-title"
        );
        assert_eq!(
            chapter_url::<TestConfig>("/obra/42/capitulo/7?slug=sample-title"),
            "https://example.org/obra/sample-title/capitulo/7"
        );
    }
}
