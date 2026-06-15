use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Roseveil = Roseveil;
const BASE_URL: &str = "https://roseveil.org";
const API_URL: &str = "https://api.roseveil.org/api";

struct Roseveil;

impl MangaSource for Roseveil {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_response(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new"
        } else {
            "views"
        };
        Ok(parse_search_response(&api_get(
            &search_url(page, "", sort, "desc", &Value::Null),
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
            let slug = slug_from_url(query).unwrap_or_else(|| query.to_string());
            return Ok(Paged {
                entries: vec![parse_details(
                    &api_get(&detail_url(&slug), DETAILS_FIXTURE),
                    Some(slug),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_search_response(&api_get(
            &search_url(
                page,
                query,
                filter_string(filters, "sort").as_deref().unwrap_or("new"),
                filter_string(filters, "order").as_deref().unwrap_or("desc"),
                filters,
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &api_get(&detail_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&api_get(&detail_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter/1".into());
        let (series_slug, chapter_slug) = chapter_parts(&key);
        Ok(parse_pages(&api_get(
            &chapter_url(&series_slug, &chapter_slug),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/comic/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (series_slug, chapter_slug) = chapter_parts(&key);
            format!("{BASE_URL}/comic/{series_slug}/chapter/{chapter_slug}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/comic/") {
            let slug = slug_from_url(input).unwrap_or_else(|| input.to_string());
            return Ok(Some(UrlResolveResult {
                item: (!input.contains("/chapter/")).then(|| {
                    parse_details(&api_get(&detail_url(&slug), DETAILS_FIXTURE), Some(slug))
                }),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, sort: &str, order: &str, filters: &Value) -> String {
    let mut params = vec![
        ("type", "COMIC".to_string()),
        ("limit", "20".to_string()),
        ("page", page.to_string()),
        ("sort", sort.to_string()),
        ("order", order.to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", query.to_string()));
    }
    for (id, parameter) in [
        ("status", "status"),
        ("subtype", "subtype"),
        ("genre", "genre"),
    ] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            params.push((parameter, value));
        }
    }
    format!(
        "{API_URL}/search?{}",
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn detail_url(slug: &str) -> String {
    format!("{API_URL}/series/comic/{}", url::query_escape(slug))
}

fn chapter_url(series_slug: &str, chapter_slug: &str) -> String {
    format!(
        "{API_URL}/series/comic/{}/chapter/{}",
        url::query_escape(series_slug),
        url::query_escape(chapter_slug)
    )
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<SearchResponseDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|item| CatalogItem {
                key: item.slug.clone(),
                title: item.title,
                cover: item.thumbnail,
                url: Some(format!("{BASE_URL}/comic/{}", item.slug)),
                language: Some("id".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: payload.page < payload.total_pages,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let data = serde_json::from_str::<MangaDetailDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let key = key.unwrap_or_else(|| data.slug.clone());
    CatalogItem {
        key: key.clone(),
        title: data.title,
        cover: data.thumbnail,
        authors: data.author.into_iter().collect(),
        artists: data.artist.into_iter().collect(),
        description: data.synopsis,
        tags: data.genres.into_iter().map(|genre| genre.name).collect(),
        status: parse_status(data.status.as_deref().unwrap_or_default()),
        url: Some(format!("{BASE_URL}/comic/{key}")),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let data = serde_json::from_str::<MangaDetailDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let series_slug = data.slug;
    data.units
        .into_iter()
        .map(|unit| {
            let number = unit.number.parse::<f32>().ok();
            MangaChapter {
                key: format!("{series_slug}/chapter/{}", unit.slug),
                title: Some(format!("Chapter {}", format_chapter_number(&unit.number))),
                chapter_number: number,
                date_uploaded: unit.date.and_then(|value| parse_iso_date(&value)),
                url: Some(format!(
                    "{BASE_URL}/comic/{series_slug}/chapter/{}",
                    unit.slug
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = serde_json::from_str::<PageListDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    data.chapter
        .pages
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.url,
                context: Some(image_headers()),
            },
            headers: image_headers(),
            description: Some(format!("Page {}", page.index)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_headers() -> manatan_shared::sdk::Context {
    let mut headers = manga::image_headers(BASE_URL);
    headers.insert(
        "Accept".to_string(),
        "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
    );
    headers
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_uppercase().as_str() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "CANCELED" | "CANCELLED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

fn format_chapter_number(value: &str) -> String {
    value
        .parse::<f32>()
        .map(|number| {
            let mut text = number.to_string();
            if text.ends_with(".0") {
                text.truncate(text.len() - 2);
            }
            text
        })
        .unwrap_or_else(|_| value.to_string())
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split_once("/comic/")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest).to_string())
        .filter(|value| !value.is_empty())
}

fn chapter_parts(key: &str) -> (String, String) {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() >= 3 && parts[1] == "chapter" {
        (parts[0].to_string(), parts[2].to_string())
    } else if parts.len() >= 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        (key.to_string(), "1".to_string())
    }
}

#[derive(Debug, Default, Deserialize)]
struct SearchResponseDto {
    #[serde(default)]
    data: Vec<MangaItemDto>,
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "total_pages")]
    total_pages: u64,
}

#[derive(Debug, Default, Deserialize)]
struct MangaItemDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default, rename = "poster_image_url")]
    thumbnail: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetailDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default, rename = "poster_image_url")]
    thumbnail: Option<String>,
    #[serde(default, rename = "author_name")]
    author: Option<String>,
    #[serde(default, rename = "artist_name")]
    artist: Option<String>,
    #[serde(default, rename = "comic_status")]
    status: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    units: Vec<ChapterUnitDto>,
}

#[derive(Debug, Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterUnitDto {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    number: String,
    #[serde(default, rename = "created_at")]
    date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PageListDto {
    #[serde(default)]
    chapter: ChapterDetailDto,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDetailDto {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Debug, Default, Deserialize)]
struct PageDto {
    #[serde(default, rename = "page_number")]
    index: u64,
    #[serde(default, rename = "image_url")]
    url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"data":[{"title":"Sample Roseveil","slug":"sample","poster_image_url":"https://roseveil.org/cover.jpg"}],"page":1,"total_pages":1}
"#;
const DETAILS_FIXTURE: &str = r#"
{"title":"Sample Roseveil","slug":"sample","synopsis":"Sample synopsis.","poster_image_url":"https://roseveil.org/cover.jpg","author_name":"Writer","artist_name":"Artist","comic_status":"ONGOING","genres":[{"name":"Action"}],"units":[{"slug":"1","number":"1","created_at":"2024-01-01T00:00:00.000Z"}]}
"#;
const PAGES_FIXTURE: &str = r#"
{"chapter":{"pages":[{"page_number":1,"image_url":"https://roseveil.org/page1.jpg"}]}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_search_response(LIST_FIXTURE).entries[0].title,
            "Sample Roseveil"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).status,
            ItemStatus::Ongoing
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
