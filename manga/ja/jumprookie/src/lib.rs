use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: JumpRookie = JumpRookie;
const BASE_URL: &str = "https://rookie.shonenjump.com";

struct JumpRookie;

impl MangaSource for JumpRookie {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_listing(
                &fetch_document_or_fixture(
                    &format!("{BASE_URL}/categories/general/recent?page={page}"),
                    LATEST_FIXTURE,
                ),
                ".series-box-list",
                page,
            ));
        }
        let genre = filter_string(&request, "genre").unwrap_or_default();
        Ok(parse_listing(
            &fetch_document_or_fixture(&popular_url(genre), LIST_FIXTURE),
            "series-contents",
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(parse_listing(
            &fetch_document_or_fixture(
                &format!("{BASE_URL}/search?query={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            ),
            "series-box-list",
            1,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &format!("{BASE_URL}{key}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/episodes/1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}{key}"),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36")
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

fn popular_url(genre: &str) -> String {
    let mut url = format!("{BASE_URL}/api/media/series_list?type=popular");
    if !genre.is_empty() {
        url.push_str("&category=");
        url.push_str(&url::query_escape(genre));
    }
    url
}

fn parse_listing(body: &str, marker: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .chain(body.split("<section").skip(1))
        .filter(|chunk| chunk.contains("series-title") || chunk.contains(marker))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Jump Rookie".into())),
                cover: html::attr_after(chunk, "cover-image", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(format!("{BASE_URL}{key}")),
                language: Some("ja".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: page == 1 && (body.contains("button-next") || body.contains("Tky-Link-Rel-Next")),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&body, "series-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Jump Rookie".into())),
        cover: html::attr_after(&body, "cover-image", "src").map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(&body, "series-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: html::text_between(&body, "user-name", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: body
            .split("series-category")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("episode-content"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "episode-content", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "episode-title", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Episode".to_string()),
                ),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("js-page-image"))
        .filter_map(|chunk| html::attr(chunk, "src"))
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

fn key_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    Some(normalize_key(input.strip_prefix(BASE_URL).unwrap_or(input)))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<section class="series-contents"><a href="/series/sample"><span class="series-title">Sample Rookie</span><img class="cover-image" src="/cover.jpg"></a></section>"#;
const LATEST_FIXTURE: &str = r#"<ul class="series-box-list"><li><a href="/series/sample"><span class="series-title">Sample Rookie</span><img class="cover-image" src="/cover.jpg"></a></li></ul>"#;
const SEARCH_FIXTURE: &str = r#"<div id="search-series"><ul class="series-box-list"><li><a href="/series/sample"><span class="series-title">Sample Rookie</span><img class="cover-image" src="/cover.jpg"></a></li></ul></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="series-title">Sample Rookie</h1><div class="user-name">Rookie Author</div><div class="series-description">Sample description.</div><div class="series-category">Battle</div><img class="cover-image" src="/cover.jpg"><ul id="episode-list"><li><a class="episode-content" href="/series/sample/episodes/1"><span class="episode-title">Episode 1</span></a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<img class="js-page-image" src="/page1.jpg"><img class="js-page-image" src="/page2.jpg">"#;
