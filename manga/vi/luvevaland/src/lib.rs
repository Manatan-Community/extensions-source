use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: LuvEvaLand = LuvEvaLand;
const DEFAULT_BASE_URL: &str = "https://luvevalands2.co";
const CDN_BASE_URL: &str = "https://picevaland.xyz/cloud";

struct LuvEvaLand;

impl MangaSource for LuvEvaLand {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(parse_popular(
                &fetch_document(&base, &format!("{base}/truyen-tranh"), LIST_FIXTURE),
                &base,
            ));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_latest(
            &fetch_document(
                &base,
                &format!("{base}/danh-sach-chuong-moi-cap-nhat?page={page}"),
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
        let filters = request.get("filters").unwrap_or(&Value::Null);
        if !query.is_empty() {
            return Ok(parse_search(
                &fetch_document(
                    &base,
                    &format!("{base}/tim-kiem?page={page}&s={}", url::query_escape(query)),
                    SEARCH_FIXTURE,
                ),
                &base,
            ));
        }
        if let Some(tag) = filter(filters, "tag") {
            return Ok(parse_latest(
                &fetch_document(
                    &base,
                    &format!("{base}/the-loai/{tag}?page={page}"),
                    LIST_FIXTURE,
                ),
                &base,
            ));
        }
        Ok(parse_search(
            &fetch_document(
                &base,
                &format!("{base}/tim-kiem?page={page}"),
                SEARCH_FIXTURE,
            ),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        let body = fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &base, auto_unlock_enabled(&request)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chap-1".into());
        let chapter_url = absolute_url(&base, &key);
        let body = fetch_document(&base, &chapter_url, PAGES_FIXTURE);
        let pages = parse_pages(&body, &base);
        if !pages.is_empty() {
            return Ok(pages);
        }
        if auto_unlock_enabled(&request) {
            if let Some(pages) = build_pages_from_pattern(&key, &base) {
                return Ok(pages);
            }
        }
        Ok(vec![manga::text_page(
            "Vui long dang nhap vao tai khoan phu hop de xem chuong nay",
        )])
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
                    .starts_with("/truyen-tranh/")
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

fn parse_popular(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-item"))
        .filter_map(|chunk| catalog_from_chunk(chunk, base))
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("book-vertical__item") || chunk.contains("book__lg"))
        .filter_map(|chunk| catalog_from_chunk(chunk, base))
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\""),
    }
}

fn parse_search(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("book__list-item") || chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| catalog_from_chunk(chunk, base))
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\""),
    }
}

fn catalog_from_chunk(chunk: &str, base: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "/truyen-tranh/", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(base, &href);
    if !key.starts_with("/truyen-tranh/") || is_chapter_key(&key) {
        return None;
    }
    let title = html::text_between(chunk, "comic-name", "</")
        .or_else(|| html::text_between(chunk, "book__lg-title", "</"))
        .or_else(|| html::text_between(chunk, "book__list-name", "</"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| normalize_thumbnail(&absolute_url(base, &image))),
        url: Some(absolute_url(base, &key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    })
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
        key: key.into(),
        title: html::text_between(body, "book__detail-name", "</")
            .or_else(|| html::text_between(body, "comic-name-detail", "</"))
            .or_else(|| html::text_between(body, "comic-name", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| normalize_thumbnail(&absolute_url(base, &image))),
        authors: text_after_label(body, "Tác giả")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/the-loai/"))
            .map(html::strip_tags)
            .filter(|value| !value.is_empty())
            .collect(),
        description: html::text_between(body, "intro-tab-content", "</div>")
            .or_else(|| html::text_between(body, "comic-intro", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(body)),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str, auto_unlock: bool) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("href")
                && (chunk.contains("/chap")
                    || chunk.contains("/chuong")
                    || chunk.contains("/chapter")
                    || chunk.contains("/mo-khoa/chap"))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(base, &href);
            if !is_chapter_key(&key) {
                return None;
            }
            let locked =
                chunk.contains("javascript") || chunk.contains("lock") || chunk.contains("khóa");
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if locked && !auto_unlock {
                    format!("Locked {title}")
                } else {
                    title
                }),
                date_uploaded: None,
                is_locked: locked && !auto_unlock,
                url: Some(absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("view-chapter")
                || chunk.contains("chapter-content")
                || chunk.contains("reading-content")
                || chunk.contains("content-chapter")
                || chunk.contains("box-chapter-content")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty() && !image.starts_with("data:image"))
        .map(|image| absolute_url(base, &image))
        .enumerate()
        .map(|(index, image)| page(index, &image, base))
        .collect()
}

fn build_pages_from_pattern(chapter_key: &str, base: &str) -> Option<Vec<MangaPage>> {
    let slug = chapter_key
        .split("/truyen-tranh/")
        .nth(1)?
        .split('/')
        .next()?;
    let chapter = chapter_number(chapter_key)?;
    let patterns = [
        (format!("{CDN_BASE_URL}/{slug}"), "c", "png"),
        (format!("{CDN_BASE_URL}/{slug}"), "c", "jpg"),
        (format!("{CDN_BASE_URL}/{slug}"), "", "png"),
        (format!("{CDN_BASE_URL}/{slug}"), "", "jpg"),
    ];
    for (base_path, prefix, ext) in patterns {
        let first = format!("{base_path}/{prefix}{chapter}/1.{ext}");
        if !is_image_url(&first, base) {
            continue;
        }
        let mut pages = Vec::new();
        for index in 1..=200 {
            let image = format!("{base_path}/{prefix}{chapter}/{index}.{ext}");
            if index > 1 && !is_image_url(&image, base) {
                break;
            }
            pages.push(page(index - 1, &image, base));
        }
        if !pages.is_empty() {
            return Some(pages);
        }
    }
    None
}

fn is_image_url(image: &str, base: &str) -> bool {
    client(base)
        .fetch("HEAD", image, None, Default::default())
        .map(|response| {
            response.status < 400
                && response.headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case("content-type") && value.starts_with("image/")
                })
        })
        .unwrap_or(false)
}

fn chapter_number(input: &str) -> Option<usize> {
    for marker in ["/chap-", "/chuong-", "/chapter-"] {
        if let Some(rest) = input.split(marker).nth(1) {
            return rest
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok();
        }
    }
    None
}

fn page(index: usize, image: &str, base: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(base)),
        },
        headers: manga::image_headers(base),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src").or_else(|| html::attr_after(input, "<img", "src"))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "<a", "</a>")
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else if lower.contains("hoàn thành") || lower.contains("truyện full") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_thumbnail(input: &str) -> String {
    for ext in [".jpg", ".jpeg", ".png", ".webp"] {
        if let Some(end) = input.to_lowercase().find(ext) {
            let prefix = &input[..end];
            if let Some(dash) = prefix.rfind('-') {
                let size = &prefix[dash + 1..];
                if size
                    .split('x')
                    .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
                {
                    return format!("{}{}", &prefix[..dash], &input[end..]);
                }
            }
        }
    }
    input.to_string()
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
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

fn auto_unlock_enabled(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("autoUnlockChapters"))
        .and_then(|value| {
            value
                .get("value")
                .unwrap_or(value)
                .as_bool()
                .or_else(|| value.as_bool())
        })
        .unwrap_or(false)
}

fn normalize_key(base: &str, input: &str) -> String {
    let without_base = input
        .trim()
        .strip_prefix(base)
        .unwrap_or(input.trim())
        .split(['?', '#'])
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", without_base.trim_start_matches('/'))
}

fn absolute_url(base: &str, input: &str) -> String {
    url::join_url(base, input)
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input
        .contains(base.trim_start_matches("https://"))
        .then(|| normalize_key(base, input))
}

fn is_chapter_key(key: &str) -> bool {
    key.contains("/chap")
        || key.contains("/chuong")
        || key.contains("/chapter")
        || key.contains("/mo-khoa/chap")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("listingId".into(), json!(listing));
        object.insert("page".into(), json!(1));
    }
    value
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

const LIST_FIXTURE: &str = r#"<div class="comic-item"><a class="comic-name" href="/truyen-tranh/sample">Sample</a><div class="comic-img"><img src="/cover-300x400.jpg"></div></div><ul class="pagination"><a rel="next" href="?page=2"></a></ul>"#;
const SEARCH_FIXTURE: &str = r#"<table class="book__list"><tr class="book__list-item"><td class="book__list-image"><a href="/truyen-tranh/sample"><img src="/cover.jpg"></a></td><td class="book__list-name"><a href="/truyen-tranh/sample">Sample</a></td></tr></table>"#;
const DETAILS_FIXTURE: &str = r#"<div class="book__detail-container"><h1 class="book__detail-name">Sample</h1><div class="book__detail-image"><img alt="Sample" src="/cover.jpg"></div><div class="book__detail-text">Tác giả: <a>Author</a></div><div class="book__detail-text">Tình trạng: Đang tiến hành</div><div class="book__detail-text">Tag: <a href="/the-loai/action">Action</a></div></div><div id="intro-tab-content">Summary</div><table class="list-chapter"><tbody><tr data-order="1"><td class="list-chapter__name"><a href="/truyen-tranh/sample/chap-1">Chapter 1</a></td><td class="list-chapter__date">01/01/2024</td></tr></tbody></table>"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><img data-src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
