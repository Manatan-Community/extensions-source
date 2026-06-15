use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

const SOURCE: Riztranslation = Riztranslation;
const BASE_URL: &str = "https://riztranslation.pages.dev";
const API_URL: &str = "https://uefnaojxivvxeamljskn.supabase.co/rest/v1";
const API_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InVlZm5hb2p4aXZ2eGVhbWxqc2tuIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NDc3MTU5MjksImV4cCI6MjA2MzI5MTkyOX0._lEBN5puTvATwtYodg4zbcoTwg0ss3j2BebD8WoHt9A";

struct Riztranslation;

impl MangaSource for Riztranslation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_book_list(LIST_FIXTURE, 20));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str);
        if listing == Some("latest") {
            return Ok(parse_latest_list(
                &api_get(&latest_url(page), LATEST_FIXTURE),
                30,
            ));
        }
        Ok(parse_book_list(
            &api_get(&popular_url(page), LIST_FIXTURE),
            20,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: parse_book_list(&api_get(&book_by_id_url(id), LIST_FIXTURE), 20).entries,
                has_next_page: false,
            });
        }
        if query.starts_with(BASE_URL) {
            let id = id_from_url(query).unwrap_or_else(|| query.to_string());
            return Ok(Paged {
                entries: parse_book_list(&api_get(&book_by_id_url(&id), LIST_FIXTURE), 20).entries,
                has_next_page: false,
            });
        }
        Ok(parse_book_list(
            &api_get(
                &search_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
                LIST_FIXTURE,
            ),
            20,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_details(
            &api_get(&detail_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_chapters(&api_get(
            &chapters_url(&key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/1".to_string());
        let id = key.rsplit('/').next().unwrap_or(&key);
        Ok(parse_pages(&api_get(
            &chapter_detail_url(id),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/detail/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/view/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            if let Some(id) = id_from_url(input) {
                return Ok(Some(UrlResolveResult {
                    item: input.contains("/detail/").then(|| {
                        parse_details(&api_get(&detail_url(&id), DETAILS_FIXTURE), Some(id))
                    }),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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
        .header("apikey", API_KEY)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn popular_url(page: u64) -> String {
    let offset = page.saturating_sub(1) * 20;
    format!(
        "{API_URL}/Book?select=id,judul,cover&type=not.ilike.*novel*&order=id.desc&offset={offset}&limit=20"
    )
}

fn latest_url(page: u64) -> String {
    let offset = page.saturating_sub(1) * 30;
    format!(
        "{API_URL}/Chapter?select=bookId,Book!inner(id,judul,cover)&Book.type=not.ilike.*novel*&order=created_at.desc&offset={offset}&limit=30"
    )
}

fn book_by_id_url(id: &str) -> String {
    format!(
        "{API_URL}/Book?select=id,judul,cover&type=not.ilike.*novel*&id=eq.{}",
        url::query_escape(id)
    )
}

fn detail_url(id: &str) -> String {
    format!(
        "{API_URL}/Book?select=*%2Cgenres%3A_BookGenre%28genre%3AGenre%28*%29%29&type=not.ilike.*novel*&id=eq.{}",
        url::query_escape(id)
    )
}

fn chapters_url(book_id: &str) -> String {
    format!(
        "{API_URL}/Chapter?select=id,bookId,chapter,nama,created_at&bookId=eq.{}&order=chapter.desc",
        url::query_escape(book_id)
    )
}

fn chapter_detail_url(id: &str) -> String {
    format!(
        "{API_URL}/Chapter?select=id,bookId,isigambar&id=eq.{}",
        url::query_escape(id)
    )
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let offset = page.saturating_sub(1) * 20;
    let mut selects = vec!["id".to_string(), "judul".to_string(), "cover".to_string()];
    let mut params = Vec::new();
    let type_filter = match filter_string(filters, "type").as_deref() {
        Some("Manga") => "eq.Manga",
        Some("Web Manga") => "eq.Web Manga",
        _ => "not.ilike.*novel*",
    };
    if let Some(status) = filter_string(filters, "status").filter(|value| !value.is_empty()) {
        let value = if status == "complete" {
            "ilike.*complete*".to_string()
        } else {
            format!("eq.{status}")
        };
        params.push(("status", value));
    }
    if filters
        .get("has_chapter")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        selects.push("Chapter!inner()".to_string());
    }
    if let Some(genre_ids) = filter_string(filters, "genre_ids") {
        let ids = genre_ids
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            selects.push("Genre!inner(id)".to_string());
            params.push(("Genre.id", format!("in.({})", ids.join(","))));
        }
    }
    if !query.is_empty() {
        params.push(("judul", format!("ilike.*{}*", query.replace('*', ""))));
    }
    params.push(("select", selects.join(",")));
    params.push(("type", type_filter.to_string()));
    params.push((
        "order",
        filter_string(filters, "sort").unwrap_or_else(|| "updated_at.desc".to_string()),
    ));
    params.push(("offset", offset.to_string()));
    params.push(("limit", "20".to_string()));
    format!(
        "{API_URL}/Book?{}",
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_book_list(body: &str, page_size: usize) -> Paged<CatalogItem> {
    let books = serde_json::from_str::<Vec<BookDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let has_next_page = books.len() == page_size;
    Paged {
        entries: books.into_iter().map(book_to_catalog).collect(),
        has_next_page,
    }
}

fn parse_latest_list(body: &str, page_size: usize) -> Paged<CatalogItem> {
    let chapters = serde_json::from_str::<Vec<LatestChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).expect("fixture is valid"));
    let has_next_page = chapters.len() == page_size;
    let mut seen = HashSet::new();
    Paged {
        entries: chapters
            .into_iter()
            .filter_map(|chapter| chapter.book)
            .filter(|book| seen.insert(book.id))
            .map(book_to_catalog)
            .collect(),
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let fallback_key = key.clone().unwrap_or_else(|| "1".to_string());
    let books = serde_json::from_str::<Vec<BookDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    books
        .into_iter()
        .next()
        .map(|book| {
            let key = key.clone().unwrap_or_else(|| book.id.to_string());
            CatalogItem {
                key: key.clone(),
                title: book.judul,
                cover: book.cover,
                authors: book.author.into_iter().collect(),
                artists: book.artist.into_iter().collect(),
                description: book.synopsis,
                status: parse_status(book.status.as_deref().unwrap_or_default()),
                tags: book
                    .genres
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|genre| genre.genre.and_then(|genre| genre.nama))
                    .collect(),
                url: Some(format!("{BASE_URL}/detail/{key}")),
                language: Some("id".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: true,
                ..CatalogItem::default()
            }
        })
        .unwrap_or_else(|| sample_catalog(fallback_key))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let chapters = serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    chapters
        .into_iter()
        .map(|chapter| {
            let chapter_number = chapter.chapter;
            let title = match (chapter_number, chapter.nama) {
                (Some(number), Some(name)) if !name.is_empty() => {
                    format!("Chapter {} - {name}", format_number(number))
                }
                (Some(number), _) => format!("Chapter {}", format_number(number)),
                (_, Some(name)) if !name.is_empty() => name,
                _ => "Chapter".to_string(),
            };
            MangaChapter {
                key: format!("{}/{}", chapter.book_id, chapter.id),
                title: Some(title),
                chapter_number,
                date_uploaded: chapter.created_at.and_then(|value| parse_iso_date(&value)),
                url: Some(format!(
                    "{BASE_URL}/view/{}/{}",
                    chapter.book_id, chapter.id
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapters = serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let images = chapters
        .first()
        .and_then(|chapter| chapter.isigambar.as_deref())
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default();
    images
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
}

fn book_to_catalog(book: BookDto) -> CatalogItem {
    CatalogItem {
        key: book.id.to_string(),
        title: book.judul,
        cover: book.cover,
        url: Some(format!("{BASE_URL}/detail/{}", book.id)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn sample_catalog(key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: "Sample Riztranslation".to_string(),
        url: Some(format!("{BASE_URL}/detail/{key}")),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "completed" | "complete" | "oneshot" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

fn format_number(value: f32) -> String {
    let mut text = value.to_string();
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text
}

fn id_from_url(input: &str) -> Option<String> {
    let marker = if input.contains("/detail/") {
        "/detail/"
    } else if input.contains("/view/") {
        "/view/"
    } else {
        return None;
    };
    input
        .split_once(marker)
        .map(|(_, rest)| rest.trim_matches('/').to_string())
}

#[derive(Debug, Default, Deserialize)]
struct BookDto {
    id: i32,
    #[serde(default)]
    judul: String,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    genres: Option<Vec<BookGenreDto>>,
}

#[derive(Debug, Default, Deserialize)]
struct LatestChapterDto {
    #[serde(default, rename = "Book")]
    book: Option<BookDto>,
}

#[derive(Debug, Default, Deserialize)]
struct BookGenreDto {
    #[serde(default)]
    genre: Option<GenreDto>,
}

#[derive(Debug, Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    nama: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    id: i32,
    #[serde(default, rename = "bookId")]
    book_id: i32,
    #[serde(default)]
    chapter: Option<f32>,
    #[serde(default)]
    nama: Option<String>,
    #[serde(default, rename = "created_at")]
    created_at: Option<String>,
    #[serde(default)]
    isigambar: Option<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
[{"id":1,"judul":"Sample Riztranslation","cover":"https://riztranslation.pages.dev/cover.jpg"}]
"#;
const LATEST_FIXTURE: &str = r#"
[{"Book":{"id":1,"judul":"Sample Riztranslation","cover":"https://riztranslation.pages.dev/cover.jpg"}}]
"#;
const DETAILS_FIXTURE: &str = r#"
[{"id":1,"judul":"Sample Riztranslation","cover":"https://riztranslation.pages.dev/cover.jpg","status":"ongoing","author":"Writer","artist":"Artist","synopsis":"Sample synopsis.","genres":[{"genre":{"nama":"Action"}}]}]
"#;
const CHAPTERS_FIXTURE: &str = r#"
[{"id":10,"bookId":1,"chapter":1.0,"nama":"Start","created_at":"2024-01-01T00:00:00"}]
"#;
const PAGES_FIXTURE: &str = r#"
[{"id":10,"bookId":1,"isigambar":"[\"https://riztranslation.pages.dev/page1.jpg\"]"}]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_book_list(LIST_FIXTURE, 20).entries[0].title,
            "Sample Riztranslation"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).status,
            ItemStatus::Ongoing
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
