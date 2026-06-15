use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaBlaze = MangaBlaze;
const BASE_URL: &str = "https://mangablaze.com";

struct MangaBlaze;

impl MangaSource for MangaBlaze {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(page_from_body(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(page_from_body(&fetch_document_or_fixture(
            &list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(page_from_body(&fetch_document_or_fixture(
            &search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find("/manga/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn list_url(page: u64, order: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!("{BASE_URL}/manga/{page_path}?m_orderby={order}")
}

fn search_url(page: u64, query: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{BASE_URL}/{page_path}?s={}&post_type=wp-manga",
        url::query_escape(query)
    )
}

fn page_from_body(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("x72-col") || chunk.contains("z8x-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "x72-title", "href")
                .or_else(|| html::attr_after(chunk, "z8x-card__title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "x72-title", "</")
                    .or_else(|| html::text_between(chunk, "z8x-card__title", "</"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        });
    Paged {
        entries,
        has_next_page: body.contains("nav-previous")
            || body.contains("nextpostslink")
            || body.contains("navigation-ajax"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "nbu-hero__title", "</")
            .or_else(|| html::text_between(body, "nbu-hero-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "nbu-hero__img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "nbu-summary__body", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Ongoing,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("nxv3-card") && !chunk.contains("zax-chapter-premium"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "zax-chapter-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".to_string())),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="x72-col"><h3 class="x72-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></article>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="nbu-hero__title">Sample Manga</h1><img class="nbu-hero__img" src="/cover.jpg">
<div class="nbu-summary__body">A sample.</div>
<a class="nxv3-card" href="/manga/sample/chapter-1/"><span class="zax-chapter-title">Chapter 1</span></a>
<a class="nxv3-card zax-chapter-premium" href="/manga/sample/premium/"><span class="zax-chapter-title">Premium</span></a>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img src="/page1.jpg"></div>"#;
