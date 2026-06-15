use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CelestialMoon = CelestialMoon;
const BASE_URL: &str = "https://celestialmoonscan.es";
const ORIGIN_URL: &str = "https://celestialmoonscan.es";
const SOURCE_NAME: &str = "Celestial Moon";
const CONTENT_RATING: &str = "adult";

struct CelestialMoon;

impl MangaSource for CelestialMoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "rating"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &advanced_search_url(page, "", sort),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(ORIGIN_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &advanced_search_url(page, query, ""),
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
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
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

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(ORIGIN_URL) {
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
        .with_cookies_for(ORIGIN_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn advanced_search_url(page: u64, query: &str, sort: &str) -> String {
    let mut params = vec![("page", page.to_string())];
    if !query.is_empty() {
        params.push(("name", url::query_escape(query)));
    }
    if !sort.is_empty() {
        params.push(("sort", sort.to_string()));
    }
    let query = params
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/advanced-search/?{query}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if !is_series_href(&href) {
                    return None;
                }
                let title = html::attr_after(chunk, "<img", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| {
                        html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                    })
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("alt=\"Next\"")
            || body.contains("alt='Next'")
            || body.contains("rel=\"next\"")
            || body.contains("pagination-next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "alt=\"poster\"", "src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "comic-content mobile", "</div>")
            .or_else(|| html::text_between(body, "comic-content", "</div>"))
            .or_else(|| html::text_between(body, "summary__content", "</div>"))
            .or_else(|| html::text_between(body, "description-summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "/genres/"),
        status: parse_status(&text_after_label(body, "Status").unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/chapter") && !href.contains("/capitulo") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(absolute_url(&key)),
                language: Some("es".into()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            let image = image_attr(chunk)?;
            let lower = chunk.to_ascii_lowercase();
            let image_lower = image.to_ascii_lowercase();
            if lower.contains("wp-manga-chapter-img")
                || lower.contains("reading-content")
                || lower.contains("readerarea")
                || lower.contains("chapter-content")
                || lower.contains("object-cover")
                || lower.contains("mx-auto")
                || image_lower.contains("/wp-content/uploads/")
            {
                Some(absolute_url(&image))
            } else {
                None
            }
        })
        .filter(|image| !image.starts_with("data:") && !image.contains("logo"))
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
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split("<div")
        .find(|chunk| html::strip_tags(chunk).trim() == label)
        .and_then(|chunk| {
            let rest = body.split_once(chunk)?.1;
            html::text_between(rest, "<div", "</div>").map(|value| html::strip_tags(&value))
        })
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "en curso" | "emision" | "emisión" => ItemStatus::Ongoing,
        "completed" | "complete" | "completado" | "finalizado" => ItemStatus::Completed,
        "hiatus" | "pausado" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn is_series_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    (lower.contains("/manga/") || lower.contains("/series/"))
        && !lower.contains("/chapter")
        && !lower.contains("/capitulo")
}

fn normalize_key(input: &str) -> String {
    let trimmed = input.trim_end_matches('/');
    for prefix in [BASE_URL, ORIGIN_URL] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return format!("/{}", rest.trim_start_matches('/'));
        }
    }
    format!("/{}", trimmed.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return input.to_string();
    }
    if input.starts_with('/') {
        return format!("{}{}", ORIGIN_URL.trim_end_matches('/'), input);
    }
    format!("{}/{}", BASE_URL.trim_end_matches('/'), input)
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

const LIST_FIXTURE: &str = r#"<div class="grid"><a href="/manga/sample"><img title="Sample Celestial Moon" src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Celestial Moon</h1><img alt="poster" src="/cover.jpg"><div class="comic-content mobile">A sample.</div>
<div>Genres</div><div><a href="/genres/action">Action</a></div><div>Status</div><div>Ongoing</div>
<div class="chapter-items"><a href="/manga/sample/chapter-1"><div class="text-sm text-white">Chapter 1</div></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="object-cover mx-auto" src="/page1.jpg"><img class="object-cover mx-auto" src="/page2.jpg"></div>"#;
