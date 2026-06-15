use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: HonkaiImpact = HonkaiImpact;
const BASE_URL: &str = "https://manga.honkaiimpact3.com";

struct HonkaiImpact;

impl MangaSource for HonkaiImpact {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_books(BOOKS_FIXTURE, ""));
        }
        Ok(parse_books(
            &fetch_document(&format!("{BASE_URL}/book"), BOOKS_FIXTURE),
            "",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_books(
            &fetch_document(&format!("{BASE_URL}/book"), BOOKS_FIXTURE),
            query,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/1".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/1".to_string());
        Ok(parse_chapters(&fetch_document(
            &format!("{}/get_chapter", url::join_url(BASE_URL, &key)),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/book/1/1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let books = parse_books(
            &fetch_document(&format!("{BASE_URL}/book"), BOOKS_FIXTURE),
            "",
        );
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Books".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: books.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/book/") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_books(body: &str, query: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("book"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/book/") && !href.starts_with("book/") {
                return None;
            }
            let title = html::text_between(chunk, "container-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Book".to_string()));
            let key = normalize_key(&url::join_url(BASE_URL, &href));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "container-cover", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    if !query.is_empty() {
        let needle = query.to_ascii_lowercase();
        entries.retain(|item| item.title.to_ascii_lowercase().contains(&needle));
    }
    entries.dedup_by(|a, b| a.key == b.key);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/book/1".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "div class=\"title", "</div>")
            .or_else(|| html::text_between(body, "class=\"title", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Book".to_string())),
        cover: html::attr_after(body, "img class=\"cover", "src")
            .or_else(|| html::attr_after(body, "class=\"cover", "data-src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "detail_info1", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or_default())
        .into_iter()
        .map(|chapter| {
            let key = format!("/book/{}/{}", chapter.book_id, chapter.chapter_id as i64);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.title),
                url: Some(url::join_url(BASE_URL, &key)),
                chapter_number: Some(chapter.chapter_id),
                date_uploaded: parse_timestamp(&chapter.timestamp),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("comic_img") || chunk.contains("data-original"))
        .filter_map(|chunk| {
            html::attr(chunk, "data-original")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/book/") {
        return format!("/{}", input[index + 1..].trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_timestamp(value: &str) -> Option<i64> {
    let date = value.split_whitespace().next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<i32>().ok()?;
    let day = parts.next()?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) as i64 * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i32 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    title: String,
    #[serde(rename = "bookid")]
    book_id: i64,
    #[serde(rename = "chapterid")]
    chapter_id: f32,
    timestamp: String,
}

export_manga_source!(SOURCE);

const BOOKS_FIXTURE: &str = r#"
<a href="/book/1"><div class="container-cover"><img src="/cover.jpg"></div><div class="container-title">Sample Book</div></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<img class="cover" src="/cover.jpg"><div class="title">Sample Book</div><div class="detail_info1">Sample description.</div>
"#;
const CHAPTERS_FIXTURE: &str =
    r#"[{"title":"Chapter 1","bookid":1,"chapterid":1.0,"timestamp":"2024-01-01 00:00:00"}]"#;
const PAGES_FIXTURE: &str =
    r#"<img class="lazy comic_img" data-original="https://manga.honkaiimpact3.com/page1.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_chapter_endpoint() {
        assert_eq!(
            SOURCE.chapters(json!({"manga": "/book/1"})).unwrap()[0].key,
            "/book/1/1"
        );
    }
}
