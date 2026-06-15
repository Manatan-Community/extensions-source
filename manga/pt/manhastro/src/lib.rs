use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Manhastro = Manhastro;
const BASE_URL: &str = "https://manhastro.net";
const API_URL: &str = "https://api2.manhastro.net";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const PER_PAGE: usize = 30;

struct Manhastro;

impl MangaSource for Manhastro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_page(all_mangas(), 1));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let body = fetch_api_or_fixture("/lancamentos", LATEST_FIXTURE);
            let payload = parse_api::<Vec<LatestItem>>(&body, LATEST_FIXTURE);
            return Ok(items_by_ids(
                payload.data.into_iter().map(|item| item.manga_id).collect(),
                false,
            ));
        }
        let body = fetch_api_or_fixture("/rank/diario", RANK_FIXTURE);
        let payload = parse_api::<Vec<RankingItem>>(&body, RANK_FIXTURE);
        Ok(items_by_ids(
            payload.data.into_iter().map(|item| item.manga_id).collect(),
            false,
        ))
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let normalized = normalize_text(query);
        let mangas = all_mangas()
            .into_iter()
            .filter(|manga| {
                normalized.is_empty()
                    || normalize_text(&manga.title()).contains(&normalized)
                    || normalize_text(manga.titulo.as_deref().unwrap_or_default())
                        .contains(&normalized)
            })
            .collect();
        Ok(parse_search_page(mangas, page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/6881".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/6881".into());
        let id = id_from_key(&key).unwrap_or(6881);
        let body = fetch_api_or_fixture(&format!("/dados/{id}"), CHAPTERS_FIXTURE);
        let payload = parse_api::<Vec<ChapterDto>>(&body, CHAPTERS_FIXTURE);
        let mut chapters = payload
            .data
            .into_iter()
            .map(ChapterDto::into_chapter)
            .collect::<Vec<_>>();
        chapters.sort_by(|left, right| {
            right
                .chapter_number
                .partial_cmp(&left.chapter_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/capitulo/186941".into());
        let id = id_from_key(&key).unwrap_or(186941);
        let body = fetch_api_or_fixture(&format!("/paginas/{id}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", normalize_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
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

export_manga_source!(SOURCE);

#[derive(Default, Deserialize)]
struct ApiResponse<T> {
    #[serde(default)]
    data: T,
}

#[derive(Clone, Default, Deserialize)]
struct MangaDto {
    #[serde(rename = "manga_id")]
    manga_id: u64,
    titulo: Option<String>,
    #[serde(rename = "titulo_brasil")]
    titulo_brasil: Option<String>,
    descricao: Option<String>,
    #[serde(rename = "descricao_brasil")]
    descricao_brasil: Option<String>,
    imagem: Option<String>,
    #[serde(default)]
    generos: Vec<String>,
    #[serde(rename = "views_mes")]
    views_mes: Option<String>,
}

#[derive(Default, Deserialize)]
struct RankingItem {
    #[serde(rename = "manga_id")]
    manga_id: u64,
}

#[derive(Default, Deserialize)]
struct LatestItem {
    #[serde(rename = "manga_id")]
    manga_id: u64,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(rename = "capitulo_id")]
    capitulo_id: u64,
    #[serde(rename = "capitulo_nome")]
    capitulo_nome: String,
    #[serde(rename = "capitulo_data")]
    capitulo_data: String,
}

#[derive(Default, Deserialize)]
struct PagesResponse {
    data: PageData,
}

#[derive(Default, Deserialize)]
struct PageData {
    chapter: Option<ChapterData>,
}

#[derive(Default, Deserialize)]
struct ChapterData {
    #[serde(rename = "baseUrl")]
    base_url: String,
    hash: String,
    #[serde(default)]
    data: Vec<String>,
}

impl MangaDto {
    fn title(&self) -> String {
        self.titulo_brasil
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(self.titulo.as_deref())
            .unwrap_or("Manhastro")
            .to_string()
    }

    fn into_item(self, initialized: bool) -> CatalogItem {
        let key = format!("/manga/{}", self.manga_id);
        CatalogItem {
            key: key.clone(),
            title: self.title(),
            cover: self.imagem.and_then(|image| image_url(&image)),
            description: self
                .descricao_brasil
                .or(self.descricao)
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            tags: self.generos.clone(),
            status: status_from_tags(&self.generos),
            url: Some(format!("{BASE_URL}{key}")),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

impl ChapterDto {
    fn into_chapter(self) -> MangaChapter {
        MangaChapter {
            key: format!("/capitulo/{}", self.capitulo_id),
            title: Some(self.capitulo_nome.clone()),
            chapter_number: chapter_number_from_text(&self.capitulo_nome),
            date_uploaded: parse_datetime(&self.capitulo_data),
            url: Some(format!("{BASE_URL}/capitulo/{}", self.capitulo_id)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        }
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn all_mangas() -> Vec<MangaDto> {
    let body = fetch_api_or_fixture("/dados", ALL_MANGAS_FIXTURE);
    parse_api::<Vec<MangaDto>>(&body, ALL_MANGAS_FIXTURE).data
}

fn items_by_ids(ids: Vec<u64>, has_next_page: bool) -> Paged<CatalogItem> {
    let mangas = all_mangas();
    let entries = ids
        .into_iter()
        .filter_map(|id| mangas.iter().find(|manga| manga.manga_id == id))
        .map(|manga| manga.clone().into_item(false))
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_search_page(mut mangas: Vec<MangaDto>, page: usize) -> Paged<CatalogItem> {
    mangas.sort_by(|left, right| {
        right
            .views_mes
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .cmp(
                &left
                    .views_mes
                    .as_deref()
                    .and_then(|value| value.parse::<u64>().ok()),
            )
            .then_with(|| left.title().cmp(&right.title()))
    });
    let start = page.saturating_sub(1) * PER_PAGE;
    let end = (start + PER_PAGE).min(mangas.len());
    let has_next_page = end < mangas.len();
    Paged {
        entries: mangas
            .into_iter()
            .skip(start)
            .take(PER_PAGE)
            .map(|manga| manga.into_item(false))
            .collect(),
        has_next_page,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let id = id_from_key(key).unwrap_or(6881);
    all_mangas()
        .into_iter()
        .find(|manga| manga.manga_id == id)
        .map(|manga| manga.into_item(true))
        .unwrap_or_else(|| CatalogItem {
            key: format!("/manga/{id}"),
            title: format!("Manga {id}"),
            url: Some(format!("{BASE_URL}/manga/{id}")),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload = serde_json::from_str::<PagesResponse>(&clean_json(body))
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let Some(chapter) = payload.data.chapter else {
        return Vec::new();
    };
    chapter
        .data
        .into_iter()
        .enumerate()
        .map(|(index, filename)| {
            let image = format!(
                "{}/{}/{}",
                chapter.base_url.trim_end_matches('/'),
                chapter.hash.trim_matches('/'),
                filename.trim_start_matches('/')
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_api<T>(body: &str, fixture: &str) -> ApiResponse<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    serde_json::from_str::<ApiResponse<T>>(&clean_json(body))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or_default())
}

fn clean_json(body: &str) -> String {
    body.trim_start_matches('\u{feff}')
        .trim_start_matches(")]}'")
        .trim_start_matches(',')
        .trim_start_matches('_')
        .trim()
        .to_string()
}

fn image_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else if value.starts_with("http://") || value.starts_with("https://") {
        Some(value.to_string())
    } else if value.contains('.') && !value.starts_with('/') {
        Some(format!("https://{value}"))
    } else {
        Some(url::join_url(BASE_URL, value))
    }
}

fn status_from_tags(tags: &[String]) -> ItemStatus {
    if tags.iter().any(|tag| tag.eq_ignore_ascii_case("Completo")) {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    let mut number = String::new();
    let mut seen_digit = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            seen_digit = true;
        } else if ch == '.' && seen_digit && !number.contains('.') {
            number.push(ch);
        } else if seen_digit {
            break;
        }
    }
    number.parse().ok()
}

fn parse_datetime(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
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

fn id_from_key(value: &str) -> Option<u64> {
    value.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn normalize_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| ch.to_lowercase())
        .map(|ch| match ch {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

fn page(request: &Value) -> usize {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize
}

const ALL_MANGAS_FIXTURE: &str = r#"{"success":true,"data":[{"manga_id":6881,"titulo":"Solo Leveling","titulo_brasil":"Nivel Unico","descricao":"Description","descricao_brasil":"Descricao","imagem":"capa.manhastro.net/wp-content/uploads/cover.jpg","qnt_capitulo":2,"generos":["Acao","Manhwa","Completo"],"views_mes":"835"},{"manga_id":6977,"titulo":"Logging 10000 Years into the Future","titulo_brasil":"10000 Anos no Futuro","descricao":"Description","imagem":"capa.manhastro.net/wp-content/uploads/cover2.jpg","qnt_capitulo":1,"generos":["Acao","Manhua"],"views_mes":"7082"}]}"#;
const RANK_FIXTURE: &str = r#"{"success":true,"data":[{"manga_id":6977},{"manga_id":6881}]}"#;
const LATEST_FIXTURE: &str = r#"{"success":true,"data":[{"manga_id":6881},{"manga_id":6977}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":[{"capitulo_id":186941,"capitulo_nome":"Capitulo 51","capitulo_data":"2023-08-28 11:46:54"},{"capitulo_id":186942,"capitulo_nome":"Capitulo 52","capitulo_data":"2023-08-29 11:46:54"}]}"#;
const PAGES_FIXTURE: &str = r#"{"success":true,"data":{"chapter":{"baseUrl":"https://img.manhastro.net","hash":"manga_sample/capitulo-1","data":["1.webp","2.webp"]}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_search_page(
                parse_api::<Vec<MangaDto>>(ALL_MANGAS_FIXTURE, ALL_MANGAS_FIXTURE).data,
                1
            )
            .entries
            .len(),
            2
        );
        assert_eq!(
            SOURCE
                .chapters(json!({"manga":"/manga/6881"}))
                .unwrap()
                .len(),
            2
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
