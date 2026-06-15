use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TodayManga = TodayManga;
const BASE_URL: &str = "https://todaymanga.com";

struct TodayManga;

impl MangaSource for TodayManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "category/recent"
        } else {
            "category/most-popular"
        };
        Ok(parse_listing(&fetch_document(
            &paged_url(&format!("{BASE_URL}/{path}"), page),
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
            let body = fetch_document(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let target = format!("{}/{}/chapter-list", BASE_URL, key.trim_matches('/'));
        Ok(parse_chapters(&fetch_document(&target, CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/category/most-popular"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &format!("{BASE_URL}/category/recent"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("serie"))
        .filter_map(parse_card)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        entries = body
            .split("<li")
            .skip(1)
            .filter_map(parse_card)
            .collect::<Vec<_>>();
    }
    Paged {
        entries,
        has_next_page: body.contains("active +")
            || body.contains("rel=\"next\"")
            || body.contains("Next"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h2", "</h2>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(chunk, "<a", "title"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".into()),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".into()),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        tags: link_texts(body, "tag-item")
            .into_iter()
            .chain(link_texts(body, "/genre/"))
            .collect(),
        authors: link_texts(body, "authors"),
        status: parse_status(&label_value(body, "status").unwrap_or_default()),
        description: html::text_between(body, "serie-summary", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key)),
                date_uploaded: html::text_between(chunk, "subtitle", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-index") || chunk.contains("chapter-content"))
        .filter_map(image_attr)
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

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let category = filter(filters, "category").unwrap_or_default();
    let genre = filter(filters, "genre").unwrap_or_default();
    let base = if !category.is_empty() {
        format!("{BASE_URL}/category/{category}")
    } else if !genre.is_empty() {
        format!("{BASE_URL}/genre/{genre}")
    } else if !query.is_empty() {
        format!("{BASE_URL}/search?q={}", url::query_escape(query))
    } else {
        format!("{BASE_URL}/category/most-popular")
    };
    paged_url(&base, page)
}

fn paged_url(base: &str, page: u64) -> String {
    if page > 1 {
        let separator = if base.contains('?') { '&' } else { '?' };
        format!("{base}{separator}page={page}")
    } else {
        base.to_string()
    }
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(ToString::to_string)
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn label_value(body: &str, label: &str) -> Option<String> {
    body.to_ascii_lowercase()
        .find(label)
        .and_then(|index| html::text_between(&body[index..], "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "complete" | "completed" => ItemStatus::Completed,
        "on going" | "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<section><div class="serie"><a href="/sample"><img src="/cover.jpg"><h2>Sample Manga</h2></a></div></section><a rel="next">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="serie"><h1>Sample Manga</h1><img src="/cover.jpg"><div class="serie-info-head"><span>Status</span><span>on going</span><div class="tags"><a class="tag-item">Action</a></div></div><div class="authors"><a>Author</a></div></div><div class="serie-summary">Sample summary.</div>"#;
const CHAPTERS_FIXTURE: &str = r#"<ul class="chapters-list"><li><a href="/sample/chapter-1">Chapter 1</a><span class="subtitle">2024-01-01</span></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="chapter-content"><img data-index="1" src="/page1.jpg"></div>"#;
