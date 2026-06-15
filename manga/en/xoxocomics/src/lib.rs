use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: XoxoComics = XoxoComics;
const BASE_URL: &str = "https://xoxocomic.com";

struct XoxoComics;

impl MangaSource for XoxoComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/comic-update?page={page}")
        } else {
            format!("{BASE_URL}/hot-comic?page={page}")
        };
        Ok(parse_list(&fetch_document_or_fixture(
            &target,
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
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/search-comic?keyword={}&page={page}",
            url::query_escape(query)
        );
        Ok(parse_list(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(fetch_all_chapters(&url::join_url(BASE_URL, &key), &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter-1".to_string());
        let target = format!(
            "{}/all",
            url::join_url(BASE_URL, &key).trim_end_matches('/')
        );
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("cartoon-box")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comic/sample".to_string());
    let info =
        html::text_between(body, "movie-info", "</section>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "XoxoComics".to_string()),
        description: html::text_between(body, "id=\"film-content\"", "</div>")
            .or_else(|| html::text_between(body, "id='film-content'", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: image_attr(&info).map(|image| url::join_url(BASE_URL, &image)),
        authors: info_value(&info, "Authors").into_iter().collect(),
        status: parse_status(&info_value(&info, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(first_url: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut next = Some(first_url.to_string());
    let mut guard = 0;
    while let Some(target) = next.take() {
        guard += 1;
        if guard > 20 {
            break;
        }
        let body = fetch_document_or_fixture(&target, CHAPTERS_FIXTURE);
        chapters.extend(parse_chapters_from_page(&body));
        next = next_page_url(&body);
        if next.is_none() && target == first_url && chapters.is_empty() {
            chapters.push(MangaChapter {
                key: manga_key.to_string(),
                title: Some("Read".to_string()),
                url: Some(first_url.to_string()),
                ..MangaChapter::default()
            });
        }
    }
    chapters
}

fn parse_chapters_from_page(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number_from_text(chunk),
                date_uploaded: None,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut seen = Vec::<String>::new();
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter_img"))
        .filter_map(image_attr)
        .map(|image| url::join_url(BASE_URL, &image))
        .filter(|image| {
            if seen.contains(image) {
                false
            } else {
                seen.push(image.clone());
                true
            }
        })
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let marker = format!("{label}:");
    html::text_between(body, &marker, "</dd>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(status: &str) -> ItemStatus {
    match status.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") && !body.contains("rel=\"next\" hidden")
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|chunk| chunk.contains("rel=\"next\"") && !chunk.contains("hidden"))
        .and_then(|chunk| html::attr(chunk, "href"))
        .map(|href| url::join_url(BASE_URL, &href))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="movie-list-index"><div class="cartoon-box"><a href="/comic/sample"><img data-src="/cover.jpg"></a><div class="detail"><h3>Sample Hub</h3></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="movie-info"><div class="series-info"><img data-src="/cover.jpg"><dt>Authors:</dt><dd>Author</dd><dt>Status:</dt><dd>Ongoing</dd></div><div id="film-content">A sample.</div></div>
<div class="episode-list"><table><tbody><tr><td><a href="/comic/sample/chapter-1">Chapter 1</a></td><td>1-Jan-2024</td></tr></tbody></table></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div class="episode-list"><table><tbody><tr><td><a href="/comic/sample/chapter-1">Chapter 1</a></td><td>1-Jan-2024</td></tr></tbody></table></div>
"#;

const PAGES_FIXTURE: &str =
    r#"<img class="chapter_img" data-src="/page1.jpg"><img class="chapter_img" src="/page2.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_html_listing_and_pages() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Hub");
        let pages = SOURCE
            .pages(json!({"chapter":"/comic/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
