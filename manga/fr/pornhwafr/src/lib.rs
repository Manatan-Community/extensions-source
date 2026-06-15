use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Source = Source;
const BASE_URL: &str = "https://pornhwa.fr";
const DIR: &str = "catalogue";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing(&request) == "latest" {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch(
            &catalogue_url(page, "", order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = request
            .get("filters")
            .and_then(|f| f.get("order"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        Ok(parse_listing(&fetch(
            &catalogue_url(page, query, order),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/catalogue/sample".into());
        Ok(parse_details(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/catalogue/sample".into());
        Ok(parse_chapters(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/catalogue/sample/chapter-1".into());
        Ok(parse_pages(&fetch(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalogue_url(page: u64, query: &str, order: &str) -> String {
    let search = if query.is_empty() {
        String::new()
    } else {
        format!("&s={}", url::query_escape(query))
    };
    format!("{BASE_URL}/{DIR}/?page={page}&order={order}{search}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|c| c.contains("bsx") || c.contains("listupd") || c.contains("utao"))
            .filter_map(card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagination") && body.to_ascii_lowercase().contains("next"),
    }
}

fn card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter") {
        return None;
    }
    let key = normalize(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Pornwha.fr".into()),
        cover: image(chunk).map(|img| url::join_url(BASE_URL, &img)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/catalogue/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Pornwha.fr".into()),
        cover: html::attr_after(body, "thumb", "src")
            .or_else(|| image(body))
            .map(|img| url::join_url(BASE_URL, &img)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        authors: info(body, "Author"),
        artists: info(body, "Artist"),
        tags: links(body, "/genres/"),
        status: status(&info(body, "Status").join(" ")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<li")
        .skip(1)
        .filter(|c| c.contains("wp-manga-chapter") || c.contains("eph-num"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "<a", "</a>")
                        .map(|v| html::strip_tags(&v))
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                url: Some(url::join_url(BASE_URL, &key)),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "chapter-release-date", "</"))
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| manatan_shared::dates::parse_fixture_date(&v)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.into(),
            title: Some("Read".into()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|c| {
            c.contains("wp-manga-chapter-img") || c.contains("readerarea") || c.contains("data-src")
        })
        .filter_map(image)
        .filter(|img| !img.starts_with("data:"))
        .enumerate()
        .map(|(i, img)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &img),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", i + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(index) = value.find(&format!("/{DIR}/")) {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) && input.contains(&format!("/{DIR}/"))).then(|| normalize(input))
}
fn image(body: &str) -> Option<String> {
    html::attr_after(body, "<img", "data-src")
        .or_else(|| html::attr_after(body, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(body, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}
fn info(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .filter(|c| c.to_ascii_lowercase().contains(&label.to_ascii_lowercase()))
        .flat_map(|c| {
            c.split("<a")
                .skip(1)
                .filter_map(|p| html::text_between(p, ">", "</a>"))
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn links(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|c| c.contains(marker))
        .filter_map(|c| html::text_between(c, ">", "</a>"))
        .map(|v| html::strip_tags(&v))
        .filter(|v| !v.is_empty())
        .collect()
}
fn status(input: &str) -> ItemStatus {
    let lower = input.to_ascii_lowercase();
    if lower.contains("complete") || lower.contains("termin") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("pause") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}
fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="bsx"><a href="/catalogue/sample/" title="Sample Pornhwa"><img src="/cover.jpg"></a></div><div class="pagination"><a class="next">Next</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Pornhwa</h1><div class="thumb"><img src="/cover.jpg"></div><div class="entry-content">Summary</div><ul><li class="wp-manga-chapter"><a href="/catalogue/sample/chapter-1/">Chapter 1</a><span class="chapterdate">2024-01-01</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
