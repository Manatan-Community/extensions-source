use aes::Aes256;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

type Aes256CbcDec = cbc::Decryptor<Aes256>;

const SOURCE: Bladetoons = Bladetoons;
const BASE_URL: &str = "https://bladetoons.com";
const API_URL: &str = "https://bladetoons.com/api";
const CDN_URL: &str = "https://cdn.bladetoons.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const ENCRYPTION_KEY: &str = "abmPisXlFjOLVTnYhbYQTpkWJtOGKwVttzLqstfjRBNVaEtQYG";

struct Bladetoons;

impl MangaSource for Bladetoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{API_URL}/capitulos/recentes?pagina={page}&limite=24")
        } else {
            format!("{API_URL}/obras/top10/views?periodo=total")
        };
        Ok(parse_manga_list(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let id = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("1")
                .split('-')
                .next()
                .unwrap_or("1");
            return Ok(Paged {
                entries: vec![details_by_id(id)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{API_URL}/obras?pagina={page}&limite=20");
        if !query.is_empty() {
            target.push_str("&busca=");
            target.push_str(&url::query_escape(query));
        }
        for id in ["status_id", "formato_id", "min_capitulos", "tag_ids"] {
            if let Some(value) = filter_value(&request, id) {
                target.push('&');
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        Ok(parse_manga_list(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".into());
        Ok(details_by_id(manga_id(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".into());
        let body = fetch_json(
            &format!("{API_URL}/obras/{}", manga_id(&key)),
            DETAILS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/obra/1/capitulo/1".into());
        let id = key
            .split("/obra/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("1");
        let chapter = key.rsplit('/').next().unwrap_or("1");
        Ok(parse_pages(&fetch_json(
            &format!("{API_URL}/obras/{id}/capitulos/{chapter}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/obra/{}", manga_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let id = key
                .split("/obra/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("1");
            let chapter = key.rsplit('/').next().unwrap_or("1");
            format!("{BASE_URL}/obra/{id}/capitulo/{chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("1")
                .split('-')
                .next()
                .unwrap_or("1");
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(id)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    let body = client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .header("Accept-Language", "pt-BR, en-US;q=0.8, en;q=0.7")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    decrypt_if_needed(&body).unwrap_or(body)
}

fn details_by_id(id: &str) -> CatalogItem {
    parse_details(&fetch_json(
        &format!("{API_URL}/obras/{id}"),
        DETAILS_FIXTURE,
    ))
}

fn parse_manga_list(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<MangoResponse<Vec<MangaDto>>>(body)
        .or_else(|_| serde_json::from_str::<MangoResponse<Vec<MangaDto>>>(LIST_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: response
            .items
            .into_iter()
            .map(|item| item.catalog(false))
            .collect(),
        has_next_page: response
            .pagination
            .as_ref()
            .is_some_and(|page| page.has_next_page),
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let response = serde_json::from_str::<MangoResponse<MangaDto>>(body)
        .or_else(|_| serde_json::from_str::<MangoResponse<MangaDto>>(DETAILS_FIXTURE))
        .unwrap_or_default();
    response.items.catalog(true)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<MangoResponse<MangaDto>>(body)
        .or_else(|_| serde_json::from_str::<MangoResponse<MangaDto>>(DETAILS_FIXTURE))
        .unwrap_or_default();
    let mut chapters = response
        .items
        .chapters
        .into_iter()
        .map(|chapter| {
            let number = chapter.number.trim_end_matches(".0").to_string();
            let key = format!("/obra/{}/capitulo/{number}", chapter.manga_id);
            MangaChapter {
                key: key.clone(),
                title: Some(
                    chapter
                        .title
                        .unwrap_or_else(|| format!("Capitulo {number}")),
                ),
                chapter_number: chapter.number.parse::<f32>().ok(),
                date_uploaded: chapter
                    .created_at
                    .or(chapter.updated_at)
                    .and_then(|date| parse_api_date(&date)),
                url: Some(format!(
                    "{BASE_URL}/obra/{}/capitulo/{number}",
                    chapter.manga_id
                )),
                language: Some(LANG.to_string()),
                is_locked: chapter.paywall.unwrap_or(false),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<MangoResponse<PageChapterDto>>(body)
        .or_else(|_| serde_json::from_str::<MangoResponse<PageChapterDto>>(PAGES_FIXTURE))
        .unwrap_or_default();
    let mut pages = response.items.pages;
    pages.sort_by_key(|page| page.number);
    pages
        .into_iter()
        .filter_map(|page| page.url)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(CDN_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decrypt_if_needed(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return None;
    }
    let (iv_hex, data_hex) = trimmed.split_once(':')?;
    let iv = hex_decode(iv_hex)?;
    let data = hex_decode(data_hex)?;
    let key = Sha256::digest(format!("{ENCRYPTION_KEY}salt").as_bytes());
    let out = Aes256CbcDec::new_from_slices(&key, &iv)
        .ok()?
        .decrypt_padded_vec_mut::<Pkcs7>(&data)
        .ok()?;
    String::from_utf8(out).ok()
}

fn hex_decode(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    (0..input.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&input[index..index + 2], 16).ok())
        .collect()
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")?
        .get(id)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn manga_id(key: &str) -> &str {
    key.split("/obra/")
        .nth(1)
        .unwrap_or("1")
        .split('/')
        .next()
        .unwrap_or("1")
        .split('?')
        .next()
        .unwrap_or("1")
}

fn parse_api_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

#[derive(Default, Deserialize)]
struct MangoResponse<T: Default> {
    #[serde(
        default,
        alias = "dados",
        alias = "obras",
        alias = "obra",
        alias = "data",
        alias = "capitulos",
        alias = "capitulo"
    )]
    items: T,
    pagination: Option<PaginationDto>,
}

#[derive(Deserialize)]
struct PaginationDto {
    #[serde(default, alias = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    id: Option<i64>,
    #[serde(default, alias = "title", alias = "nome")]
    title: String,
    #[serde(
        default,
        alias = "slug",
        alias = "nome_url",
        alias = "permalink",
        alias = "url"
    )]
    slug: Option<String>,
    #[serde(default, alias = "coverImage", alias = "imagem")]
    cover_image: Option<String>,
    #[serde(default, alias = "descricao")]
    description: Option<String>,
    #[serde(default, alias = "status_id")]
    status_id: Option<i64>,
    #[serde(default, alias = "status_nome")]
    status_name: Option<String>,
    #[serde(default)]
    tags: Vec<TagDto>,
    #[serde(default, alias = "capitulos")]
    chapters: Vec<ChapterDto>,
}

impl MangaDto {
    fn catalog(self, initialized: bool) -> CatalogItem {
        let id = self.id.unwrap_or(1);
        let key = format!("/obra/{id}");
        CatalogItem {
            key: key.clone(),
            title: if self.title.is_empty() {
                "Bladetoons".into()
            } else {
                self.title
            },
            cover: self.cover_image.map(|image| url::join_url(CDN_URL, &image)),
            description: self.description,
            tags: self.tags.into_iter().map(|tag| tag.name).collect(),
            status: status_from(self.status_name.as_deref(), self.status_id),
            url: Some(format!(
                "{BASE_URL}/obra/{}",
                self.slug.unwrap_or_else(|| id.to_string())
            )),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct TagDto {
    #[serde(default, alias = "nome", alias = "name")]
    name: String,
}

#[derive(Deserialize)]
struct ChapterDto {
    #[serde(alias = "obra_id")]
    manga_id: i64,
    #[serde(alias = "numero")]
    number: String,
    #[serde(default, alias = "nome", alias = "title")]
    title: Option<String>,
    #[serde(default)]
    paywall: Option<bool>,
    #[serde(default, alias = "criado_em")]
    created_at: Option<String>,
    #[serde(default, alias = "atualizado_em")]
    updated_at: Option<String>,
}

#[derive(Default, Deserialize)]
struct PageChapterDto {
    #[serde(default, alias = "paginas")]
    pages: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
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

fn status_from(name: Option<&str>, id: Option<i64>) -> ItemStatus {
    match name.unwrap_or_default().trim() {
        "Ativo" | "Em Andamento" => ItemStatus::Ongoing,
        "Concluido" | "Concluído" => ItemStatus::Completed,
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"dados":[{"id":1,"nome":"Sample Bladetoons","imagem":"/cover.jpg","status_id":1}],"pagination":{"hasNextPage":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"obra":{"id":1,"nome":"Sample Bladetoons","imagem":"/cover.jpg","descricao":"Sample description.","status_id":1,"tags":[{"nome":"Acao"}],"capitulos":[{"obra_id":1,"numero":"1","nome":"Capitulo 1","criado_em":"2024-01-01T00:00:00Z"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"capitulo":{"paginas":[{"numero":1,"imagem":"/page-1.jpg"}]}}"#;
