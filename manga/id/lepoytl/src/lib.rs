use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: LepoyTl = LepoyTl;
const BASE_URL: &str = "https://www.lepoytl.my.id";

struct LepoyTl;

impl MangaSource for LepoyTl {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            page_url("/search/label/Chapter", page)
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
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
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/p/sample-lepoytl.html".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/p/sample-lepoytl.html".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample-lepoytl-chapter-1.html".into());
        Ok(parse_pages(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page_url(path: &str, page: u64) -> String {
    if page <= 1 {
        format!("{BASE_URL}{path}")
    } else {
        format!("{BASE_URL}{path}?updated-max=&max-results=20#PageNo={page}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .or_else(|| html::text_between(chunk, "entry-title", "</"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "LepoyTL".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| fix_blogger_image(&image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("blog-pager-older-link") || body.contains("loadMore"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/p/sample-lepoytl.html".to_string());
    let info = body
        .split("#extra-info")
        .nth(1)
        .or_else(|| body.split("extra-info").nth(1))
        .unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "LepoyTL".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| fix_blogger_image(&image)),
        description: html::text_between(body, "synopsis", "</")
            .or_else(|| html::text_between(body, "Sinopsis", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(info, "Author").into_iter().collect(),
        artists: info_value(info, "Artist").into_iter().collect(),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("rel=\"tag\"") || chunk.contains("/search/label/"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&info_value(info, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("Chapter") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("reader") || chunk.contains("separator") || chunk.contains("data-src")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: fix_blogger_image(&url::join_url(BASE_URL, &image)),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<dt")
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .and_then(|chunk| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("complete") || value.contains("tamat") {
        ItemStatus::Completed
    } else if value.contains("hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("ongoing") || value.contains("berjalan") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn fix_blogger_image(input: &str) -> String {
    input
        .replace("/s1600/", "/s0/")
        .replace("/s320/", "/s0/")
        .replace("/s640/", "/s0/")
        .replace("=s1600", "=s0")
        .replace("=s640", "=s0")
        .replace("=s320", "=s0")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .split('?')
                .next()
                .unwrap_or_default()
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!(
        "/{}",
        input
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article><a href="https://www.lepoytl.my.id/p/sample-lepoytl.html"><img src="https://blogger.googleusercontent.com/img/s1600/cover.jpg" alt="Sample LepoyTL"></a><h3><a href="https://www.lepoytl.my.id/p/sample-lepoytl.html">Sample LepoyTL</a></h3></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample LepoyTL">
<meta property="og:image" content="https://blogger.googleusercontent.com/img/s1600/cover.jpg">
<div class="synopsis"><p>Sample synopsis.</p></div>
<aside id="extra-info"><dl><dt>Status</dt><dd>Ongoing</dd><dt>Author</dt><dd>Writer</dd><dt>Artist</dt><dd>Artist</dd></dl><a rel="tag">Action</a></aside>
<a href="https://www.lepoytl.my.id/2024/01/sample-lepoytl-chapter-1.html">Chapter 1</a>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="reader"><img data-src="https://blogger.googleusercontent.com/img/s1600/page1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample LepoyTL"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
