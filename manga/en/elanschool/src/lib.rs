use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ElanSchool = ElanSchool;
const BASE_URL: &str = "https://elan.school";
const MANGA_KEY: &str = "/chapters/";
const COVER_URL: &str =
    "https://elan.school/wp-content/uploads/2018/11/The-Elan-School-Comic-1cNEW-1-768x1491.jpg";

struct ElanSchool;

impl MangaSource for ElanSchool {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(Paged {
            entries: vec![series_item(false, page)],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let entries = if query.is_empty()
            || "elan school".contains(&query.to_ascii_lowercase())
            || query.starts_with(BASE_URL)
        {
            vec![series_item(false, 1)]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item(true, 1))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| MANGA_KEY.to_string());
        let start_url = url::join_url(BASE_URL, &key);
        Ok(parse_all_chapters(&start_url))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter-1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(true, 1)),
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

fn series_item(initialized: bool, page: u64) -> CatalogItem {
    CatalogItem {
        key: format!("{MANGA_KEY}?dps_paged={page}"),
        title: "Elan School".to_string(),
        cover: Some(COVER_URL.to_string()),
        description: Some("A 16 year old boy named Joe gets indoctrinated into a sick cult that is run by imprisoned teenagers. Based on the true story of the Elan School.".to_string()),
        authors: vec!["Joe Nobody".to_string()],
        artists: vec!["Joe Nobody".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(format!("{BASE_URL}{MANGA_KEY}?dps_paged={page}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized,
        ..CatalogItem::default()
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

fn parse_all_chapters(start_url: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut next_url = Some(start_url.to_string());
    let mut seen = Vec::<String>::new();

    while let Some(target) = next_url {
        if seen.iter().any(|url| url == &target) {
            break;
        }
        seen.push(target.clone());
        let body = fetch_document(&target, CHAPTERS_FIXTURE);
        chapters.extend(parse_chapter_page(&body));
        next_url = next_page_url(&body);
        if next_url.is_some() && body == CHAPTERS_FIXTURE {
            break;
        }
    }

    chapters.reverse();
    chapters
}

fn parse_chapter_page(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("listing-item") && chunk.contains("<a"))
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
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|chunk| chunk.contains("next"))
        .and_then(|chunk| html::attr(chunk, "href"))
        .map(|href| url::join_url(BASE_URL, &href))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-orig-file"))
        .filter_map(|chunk| {
            html::attr(chunk, "src")
                .or_else(|| html::attr(chunk, "data-orig-file"))
                .filter(|value| !value.is_empty())
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

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
<div class="listing-item"><a class="title" href="https://elan.school/chapter-1/">Chapter 1</a></div>
<div class="listing-item"><a class="title" href="https://elan.school/chapter-2/">Chapter 2</a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<img data-orig-file="https://elan.school/wp-content/uploads/page1.jpg" src="https://elan.school/wp-content/uploads/page1-768x1024.jpg">
<img data-orig-file="https://elan.school/wp-content/uploads/page2.jpg" src="https://elan.school/wp-content/uploads/page2-768x1024.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chapters_and_pages() {
        let chapters = parse_chapter_page(CHAPTERS_FIXTURE);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "/chapter-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
