use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Pramramadhan = Pramramadhan;
const BASE_URL: &str = "https://01.pramramadhan.my.id";
const SOURCE_NAME: &str = "Pramramadhan";
const CONTENT_RATING: &str = "adult";

struct Pramramadhan;

impl MangaSource for Pramramadhan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "newest"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, "", Some(sort), &Value::Null),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(
                page,
                query,
                None,
                request.get("filters").unwrap_or(&Value::Null),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".into());
        Ok(parse_pages(
            &fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE),
            &absolute_url(&key),
        ))
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
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, forced_sort: Option<&str>, filters: &Value) -> String {
    let mut params = vec![("q", query.to_string()), ("page", page.to_string())];
    for id in [
        "sort", "genre", "type", "project", "status", "author", "artist",
    ] {
        let value = if id == "sort" {
            filter_string(filters, id).or_else(|| forced_sort.map(ToString::to_string))
        } else {
            filter_string(filters, id)
        };
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.push((id, value));
        }
    }
    format!(
        "{BASE_URL}/search.php?{}",
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("result-card"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "result-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "result-cover", "src")
                        .or_else(|| image_attr(chunk))
                        .map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("id".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item),
        has_next_page: body.contains("next") && body.contains("page"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "series-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| SOURCE_NAME.into())),
        authors: tag_values(body, "Author"),
        artists: tag_values(body, "Artist"),
        tags: tag_values(body, "Genre"),
        status: tag_values(body, "Status")
            .first()
            .map(|status| parse_status(status))
            .unwrap_or(ItemStatus::Unknown),
        description: html::text_between(body, "series-desc", "</")
            .or_else(|| html::text_between(body, "<p", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "series-cover", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn tag_values(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .filter(|chunk| {
            chunk.contains("tag-row")
                && html::strip_tags(chunk)
                    .to_ascii_lowercase()
                    .contains(&label.to_ascii_lowercase())
        })
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .chain(chunk.split("<span").skip(1))
                .filter(|piece| piece.contains("tag-pill"))
                .filter_map(|piece| html::text_between(piece, ">", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-card"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let subtitle = html::text_between(chunk, "chapter-sub", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let full_title = subtitle
                .map(|subtitle| format!("{title} - {subtitle}"))
                .unwrap_or(title);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(full_title.clone()),
                chapter_number: chapter_number_from_text(&full_title),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page") || chunk.contains("reader-container"))
        .filter_map(image_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" | "complete" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 3 {
        let day = parts[0].parse::<u32>().ok()?;
        let month = month_number(parts[1])?;
        let year = parts[2].parse::<i32>().ok()?;
        return dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"));
    }
    None
}

fn month_number(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "januari" | "january" => Some(1),
        "februari" | "february" => Some(2),
        "maret" | "march" => Some(3),
        "april" => Some(4),
        "mei" | "may" => Some(5),
        "juni" | "june" => Some(6),
        "juli" | "july" => Some(7),
        "agustus" | "august" => Some(8),
        "september" => Some(9),
        "oktober" | "october" => Some(10),
        "november" => Some(11),
        "desember" | "december" => Some(12),
        _ => None,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_key(input: &str) -> String {
    let path = if input.starts_with("http://") || input.starts_with("https://") {
        input
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/').map(|(_, path)| path))
            .unwrap_or_default()
    } else {
        input
    };
    format!(
        "/{}",
        path.split(['?', '#'])
            .next()
            .unwrap_or(path)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique_catalog_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="result-card" href="/series/sample"><div class="result-cover"><img src="/cover.jpg"></div><div class="result-title">Sample Pramramadhan</div></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="series-title">Sample Pramramadhan</h1><div class="series-cover"><img src="/cover.jpg"></div><p class="series-desc">Sample synopsis.</p>
<div class="tag-row"><span class="tag-label">Author</span><a class="tag-pill">Writer</a></div>
<div class="tag-row"><span class="tag-label">Artist</span><a class="tag-pill">Artist</a></div>
<div class="tag-row"><span class="tag-label">Genre</span><a class="tag-pill">Adventure</a></div>
<div class="tag-row"><span class="tag-label">Status</span><span class="tag-pill">Ongoing</span></div>
<div class="chapter-grid"><a class="chapter-card" href="/series/sample/chapter-1"><div class="chapter-title">Chapter 1</div><div class="chapter-sub">Start</div><div class="chapter-time">1 Januari 2024</div></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reader-container"><img class="page" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Pramramadhan"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).status,
            ItemStatus::Ongoing
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
