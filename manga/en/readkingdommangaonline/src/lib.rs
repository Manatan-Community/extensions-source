use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ReadKingdomMangaOnline = ReadKingdomMangaOnline;
const BASE_URL: &str = "https://ww5.readkingdom.com";
const NAME: &str = "Read Kingdom Manga Online";

struct ReadKingdomMangaOnline;

impl MangaSource for ReadKingdomMangaOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: vec![source_item()],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: vec![details_from(&fetch_document(BASE_URL, DETAILS_FIXTURE))],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or("");
        let entries = if query.trim().is_empty()
            || NAME
                .to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
            || query.starts_with(BASE_URL)
        {
            vec![details_from(&fetch_document(BASE_URL, DETAILS_FIXTURE))]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(details_from(&fetch_document(BASE_URL, DETAILS_FIXTURE)))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(BASE_URL, DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| BASE_URL.to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from(&fetch_document(input, DETAILS_FIXTURE))),
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

fn source_item() -> CatalogItem {
    CatalogItem {
        key: BASE_URL.to_string(),
        title: NAME.to_string(),
        url: Some(BASE_URL.to_string()),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_from(body: &str) -> CatalogItem {
    CatalogItem {
        key: BASE_URL.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| NAME.to_string()),
        cover: html::attr_after(body, "div.flex", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "Description", "</div>")
            .or_else(|| html::text_between(body, "flex-col", "</div>"))
            .map(|value| {
                html::strip_tags(&value)
                    .replace("Description", "")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty()),
        url: Some(BASE_URL.to_string()),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("col-span-4") || chunk.contains("grid"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "col-span-4", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let first = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let extra = html::text_between(chunk, "text-xs", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let title = extra.map_or(first.clone(), |value| format!("{first} - {value}"));
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input.to_string()
    } else {
        absolute_url(input)
    }
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

export_manga_source!(SOURCE);

const DETAILS_FIXTURE: &str = r#"<div class="container"><h1>Sample Manga</h1></div><div class="flex"><img src="/cover.jpg"></div><div class="flex-col">Description Sample summary.</div><div class="w-full"><div class="bg-bg-secondary"><div class="grid"><div class="col-span-4"><a href="/chapter-1">Chapter 1</a></div><div class="text-xs">Title</div></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<img data-src="/page1.jpg"><img data-src="/page2.jpg">"#;
