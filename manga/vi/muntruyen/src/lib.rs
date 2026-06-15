use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: MunTruyen = MunTruyen;
const DEFAULT_BASE_URL: &str = "https://munhihi.icu";

struct MunTruyen;

impl MangaSource for MunTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "views"
        } else {
            "updated"
        };
        Ok(parse_filter_page(
            &fetch_document(
                &base,
                &build_filter_url(&base, page, sort, &Value::Null),
                LIST_FIXTURE,
            ),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(&base, query) {
            return Ok(Paged {
                entries: vec![details_by_key(&base, &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if !query.is_empty() {
            let page_path = if page > 1 {
                format!("page/{page}/")
            } else {
                String::new()
            };
            let target = format!("{base}/{page_path}?s={}", url::query_escape(query));
            return Ok(parse_search_page(
                &fetch_document(&base, &target, SEARCH_FIXTURE),
                &base,
            ));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        Ok(parse_filter_page(
            &fetch_document(
                &base,
                &build_filter_url(
                    &base,
                    page,
                    filter(filters, "sort").unwrap_or("updated"),
                    filters,
                ),
                LIST_FIXTURE,
            ),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters_paginated(&base, &absolute_url(&base, &key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chap-1".into());
        let chapter_url = absolute_url(&base, &key);
        let pages = parse_pages(&fetch_document(&base, &chapter_url, PAGES_FIXTURE), &base);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, "popular"))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, "latest"))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(&base, input) {
            return Ok(Some(UrlResolveResult {
                item: key
                    .contains("/truyen/")
                    .then(|| details_by_key(&base, &key)),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn build_filter_url(base: &str, page: u64, sort: &str, filters: &Value) -> String {
    let page_path = if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    };
    let mut pairs = vec![
        "type=comic".to_string(),
        format!(
            "status={}",
            url::query_escape(filter(filters, "status").unwrap_or(""))
        ),
        format!(
            "age_rating={}",
            url::query_escape(filter(filters, "ageRating").unwrap_or(""))
        ),
        format!(
            "author={}",
            url::query_escape(filter(filters, "author").unwrap_or(""))
        ),
        format!(
            "team={}",
            url::query_escape(filter(filters, "team").unwrap_or(""))
        ),
        "rating_min=0".to_string(),
        "rating_max=6".to_string(),
        format!("sort={}", url::query_escape(sort)),
    ];
    for genre in multi_filter(filters, "genres") {
        pairs.push(format!("genre[]={}", url::query_escape(&genre)));
    }
    format!("{base}/bo-loc-nang-cao/{page_path}?{}", pairs.join("&"))
}

fn parse_search_page(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen/") && !chunk.contains("/truyen/truyen-chu"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(item(
                base,
                key,
                title,
                image_attr(chunk).map(|image| normalize_image(base, &image)),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("aria-label=\"Trang sau\"")
            || body.contains("aria-label='Trang sau'"),
    }
}

fn parse_filter_page(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<h2")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen/") {
                return None;
            }
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(item(
                base,
                key,
                title,
                image_near(body, &href).map(|image| normalize_image(base, &image)),
            ))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("aria-label=\"Trang sau\"")
            || body.contains("aria-label='Trang sau'"),
    }
}

fn item(base: &str, key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(absolute_url(base, &key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(base, &absolute_url(base, key), DETAILS_FIXTURE),
        base,
        key,
    )
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(base, key),
        title: html::text_between(body, "manga-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "story-cover-wrap", "src")
            .or_else(|| image_attr(body))
            .map(|image| normalize_image(base, &image)),
        tags: link_texts_after(body, "genre-tags"),
        description: html::text_between(body, "manga-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(
            &html::text_between(body, "manga-status", "</")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_paginated(base: &str, manga_url: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for page in 1..=50 {
        let body = fetch_document(
            base,
            &format!("{}/chap/page/{page}/", manga_url.trim_end_matches('/')),
            CHAPTERS_FIXTURE,
        );
        let page_chapters = parse_chapter_page(&body, base);
        let before = chapters.len();
        for chapter in page_chapters {
            if !chapters
                .iter()
                .any(|seen: &MangaChapter| seen.key == chapter.key)
            {
                chapters.push(chapter);
            }
        }
        if chapters.len() == before || !has_next_chapter_page(&body, page) {
            break;
        }
    }
    chapters
}

fn parse_chapter_page(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(base, &href);
            let raw_title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(parse_chapter_name(&raw_title)),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("src="))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .map(|image| normalize_image(base, &image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.clone(),
                context: Some(manga::image_headers(base)),
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_chapter_name(raw: &str) -> String {
    raw.to_lowercase()
        .find("chap")
        .map(|index| raw[index..].trim().to_string())
        .unwrap_or_else(|| raw.trim().to_string())
}

fn has_next_chapter_page(body: &str, page: u64) -> bool {
    body.contains("chap/page/") && body.contains(&format!(">{}<", page + 1))
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("trọn bộ") {
        ItemStatus::Completed
    } else if lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(manatan_shared::dates::parse_ymd)
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("overrideBaseUrl"))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| value.starts_with("http"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn normalize_key(base: &str, input: &str) -> String {
    if input.starts_with("http") {
        input
            .trim_start_matches(base)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(base: &str, input: &str) -> String {
    url::join_url(base, input)
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input
        .starts_with(base)
        .then(|| normalize_key(base, input))
        .filter(|key| key.contains("/truyen/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
}

fn multi_filter(filters: &Value, id: &str) -> Vec<String> {
    match filters.get(id) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.get("value").unwrap_or(value).as_str())
            .map(str::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(value) => value
            .get("value")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn image_near(body: &str, href: &str) -> Option<String> {
    let index = body.find(href)?;
    let start = index.saturating_sub(800);
    let end = (index + 800).min(body.len());
    image_attr(&body[start..end])
}

fn normalize_image(base: &str, input: &str) -> String {
    absolute_url(base, input).replace("-150x150.", ".")
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|index| {
            body[index..]
                .split("<a")
                .skip(1)
                .take(50)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("listingId".into(), json!(listing));
        object.insert("page".into(), json!(1));
    }
    value
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div><img src="/cover.jpg" alt="Ảnh bìa"><h2><a href="/truyen/sample">Sample</a></h2></div>"#;
const SEARCH_FIXTURE: &str =
    r#"<article><a href="/truyen/sample"><img src="/cover.jpg"></a><h2>Sample</h2></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="manga-title">Sample</h1><div class="story-cover-wrap"><a class="story-cover"><img src="/cover.jpg"></a></div><div id="manga-status">Đang tiến hành</div><div id="genre-tags"><a href="/the-loai/action">Action</a></div><div id="manga-description">Summary</div>"#;
const CHAPTERS_FIXTURE: &str = r#"<div class="chapter-item"><a href="/truyen/sample/chap-1"><h3>Sample Chap 1</h3></a><time datetime="2024-01-01T00:00:00+07:00"></time></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="chapter-content"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
