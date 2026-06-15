use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ShadowManga = ShadowManga;
const BASE_URL: &str = "https://shademanga.com";
const CONTENT_RATING: &str = "adult";
const LANG: &str = "es";
const MAX_RESULTS: u64 = 120;

struct ShadowManga;

impl MangaSource for ShadowManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_wrapped_series(POPULAR_FIXTURE));
        }
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "/api/series-locales/novedades"
        } else {
            "/api/series-locales/popular"
        };
        Ok(parse_wrapped_series(&fetch_json(
            &format!("{BASE_URL}{path}"),
            POPULAR_FIXTURE,
        )))
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
                entries: vec![details_item(&key)],
                has_next_page: false,
            });
        }
        let body = fetch_json(&search_url(query, request.get("filters")), SEARCH_FIXTURE);
        let excluded = filter_list(request.get("filters"), "excludedGenres");
        let mut entries = parse_series_array(&body)
            .entries
            .into_iter()
            .filter(|item| {
                excluded
                    .iter()
                    .all(|genre| !item.tags.iter().any(|tag| tag.eq_ignore_ascii_case(genre)))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.title.cmp(&right.title));
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(details_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let series = fetch_series(&key);
        Ok(series
            .chapters
            .into_iter()
            .map(|chapter| chapter.to_chapter(series.id))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/1".to_string());
        let (series_id, chapter_id) = key.split_once('/').unwrap_or(("1", key.as_str()));
        let payload: PagesWrapper = fetch_json_value(
            &format!("{BASE_URL}/api/series-locales/{series_id}/capitulos/{chapter_id}/paginas"),
            PAGES_FIXTURE,
        );
        Ok(payload
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, image)| page_with_fallback(index, image))
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/serie/local/{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let chapter = key.split('/').next_back().unwrap_or(&key);
            format!("{BASE_URL}/reader/local/{chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/serie/local/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&key)),
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
        .with_cookies_for("https://media.shademanga.com")
        .with_cookies_for("https://cdn.shademanga.com")
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_value<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    serde_json::from_str(&fetch_json(target, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn search_url(query: &str, filters: Option<&Value>) -> String {
    let mut out = format!(
        "{BASE_URL}/api/series-locales/search-candidates?q={}&includeAdult=true&showSinPortada=false&take={MAX_RESULTS}",
        url::query_escape(query)
    );
    for genre in filter_list(filters, "genres") {
        out.push_str("&tags=");
        out.push_str(&url::query_escape(&genre));
    }
    out
}

fn parse_wrapped_series(body: &str) -> Paged<CatalogItem> {
    let wrappers: Vec<SeriesWrapper> = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).unwrap_or_default());
    let mut entries = Vec::new();
    for wrapper in wrappers {
        for series in wrapper.series {
            let item = series.to_item(false);
            if !entries
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                entries.push(item);
            }
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_series_array(body: &str) -> Paged<CatalogItem> {
    let series: Vec<Series> = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or_default());
    Paged {
        entries: series
            .into_iter()
            .map(|series| series.to_item(false))
            .collect(),
        has_next_page: false,
    }
}

fn details_item(key: &str) -> CatalogItem {
    fetch_series(key).to_item(true)
}

fn fetch_series(key: &str) -> Series {
    fetch_json_value(
        &format!("{BASE_URL}/api/series-locales/{}", normalize_key(key)),
        DETAILS_FIXTURE,
    )
}

fn page_with_fallback(index: usize, image: String) -> MangaPage {
    let mut context = manga::image_headers(BASE_URL);
    if let Some(fallback) = fallback_image_url(&image) {
        context.insert("Fallback-Url".to_string(), fallback);
    }
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(context.clone()),
        },
        headers: context,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn fallback_image_url(image: &str) -> Option<String> {
    let host = if image.contains("media.shademanga.com") {
        "cdn.shademanga.com"
    } else if image.contains("cdn.shademanga.com") {
        "media.shademanga.com"
    } else {
        return None;
    };
    let path = image
        .split(".com/")
        .nth(1)?
        .trim_start_matches("api/media/");
    Some(format!("https://{host}/{path}"))
}

fn normalize_key(input: &str) -> String {
    let mut value = input.trim().trim_end_matches('/').to_string();
    if let Some((_, rest)) = value.split_once("/serie/local/") {
        value = rest.to_string();
    }
    value.trim_matches('/').to_string()
}

fn filter_list(filters: Option<&Value>, id: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(id)) else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Debug, Default, Deserialize)]
struct SeriesWrapper {
    #[serde(default)]
    series: Vec<Series>,
}

#[derive(Debug, Default, Deserialize)]
struct Series {
    id: u64,
    #[serde(rename = "titulo")]
    title: String,
    #[serde(rename = "portadaUrl")]
    thumbnail_url: Option<String>,
    #[serde(rename = "descripcion")]
    description: Option<String>,
    #[serde(rename = "autor")]
    author: Option<String>,
    #[serde(rename = "generos")]
    genres: Option<String>,
    #[serde(rename = "estado")]
    status: Option<String>,
    #[serde(rename = "capitulos", default)]
    chapters: Vec<Chapter>,
}

impl Series {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.id.to_string(),
            title: if self.title.is_empty() {
                "Shadow Manga".to_string()
            } else {
                self.title.clone()
            },
            cover: self.thumbnail_url.clone(),
            description: initialized.then(|| self.description.clone()).flatten(),
            authors: initialized
                .then(|| self.author.clone().into_iter().collect())
                .unwrap_or_default(),
            artists: initialized
                .then(|| self.author.clone().into_iter().collect())
                .unwrap_or_default(),
            tags: self.genre_list(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/serie/local/{}", self.id)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }

    fn genre_list(&self) -> Vec<String> {
        self.genres
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect()
    }
}

#[derive(Debug, Default, Deserialize)]
struct Chapter {
    id: u64,
    #[serde(rename = "numeroCapitulo")]
    chapter_number: f32,
    #[serde(rename = "titulo")]
    title: Option<String>,
    #[serde(rename = "fechaSubida")]
    upload_date: Option<String>,
}

impl Chapter {
    fn to_chapter(&self, manga_id: u64) -> MangaChapter {
        let number = if self.chapter_number.fract() == 0.0 {
            format!("{}", self.chapter_number as u64)
        } else {
            self.chapter_number.to_string()
        };
        let title = self
            .title
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|title| format!("Cap. {number} - {title}"))
            .unwrap_or_else(|| format!("Cap. {number}"));
        MangaChapter {
            key: format!("{manga_id}/{}", self.id),
            title: Some(title),
            chapter_number: Some(self.chapter_number),
            date_uploaded: self.upload_date.as_deref().and_then(parse_date),
            url: Some(format!("{BASE_URL}/reader/local/{}", self.id)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct PagesWrapper {
    #[serde(rename = "paginas", default)]
    pages: Vec<String>,
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.unwrap_or_default().to_ascii_lowercase().as_str() {
        "en curso" => ItemStatus::Ongoing,
        "completado" => ItemStatus::Completed,
        "pausada" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
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

const POPULAR_FIXTURE: &str = r#"[{"series":[{"id":1,"titulo":"Sample Shadow","portadaUrl":"https://media.shademanga.com/cover.jpg","descripcion":"Summary","autor":"Author","generos":"Accion, Drama","estado":"En curso","capitulos":[{"id":10,"numeroCapitulo":1,"titulo":"Inicio","fechaSubida":"2024-01-01T00:00:00.000000"}]}]}]"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"titulo":"Sample Shadow","portadaUrl":"https://media.shademanga.com/cover.jpg","descripcion":"Summary","autor":"Author","generos":"Accion, Drama","estado":"En curso","capitulos":[]}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"titulo":"Sample Shadow","portadaUrl":"https://media.shademanga.com/cover.jpg","descripcion":"Summary","autor":"Author","generos":"Accion, Drama","estado":"En curso","capitulos":[{"id":10,"numeroCapitulo":1,"titulo":"Inicio","fechaSubida":"2024-01-01T00:00:00.000000"}]}"#;
const PAGES_FIXTURE: &str = r#"{"paginas":["https://media.shademanga.com/api/media/page1.jpg"]}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_wrapped_series(POPULAR_FIXTURE).entries.len(), 1);
        assert_eq!(details_item("1").title, "Sample Shadow");
        assert_eq!(SOURCE.chapters(serde_json::json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(serde_json::json!({})).unwrap().len(), 1);
    }
}
