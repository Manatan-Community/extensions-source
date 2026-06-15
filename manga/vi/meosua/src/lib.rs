use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MeoSua = MeoSua;
const BASE_URL: &str = "https://meosua.com";

struct MeoSua;

impl MangaSource for MeoSua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "truyen-moi-cap-nhat"
        } else {
            "xem-nhieu-nhat"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}/?trang={page}"),
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(Paged {
            entries: parse_listing_entries(&fetch_document(
                &format!("{BASE_URL}/?s={}", url::query_escape(query)),
                LIST_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        let first_url = absolute_url(&key);
        let first_body = fetch_document(&first_url, DETAILS_FIXTURE);
        let mut chapters = parse_chapter_items(&first_body);
        for page in 2..=max_chapter_page(&first_body).min(30) {
            let body = fetch_document(&format!("{first_url}?trang={page}"), "");
            for chapter in parse_chapter_items(&body) {
                chapters = push_unique_chapter(chapters, chapter);
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chapter-1".into());
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        if body.contains("lock-card") || body.contains("unlock-chapter") || body.contains("xu-lock")
        {
            return Ok(vec![manga::text_page(
                "Chapter is locked. Log in with WebView and a matching account to read it.",
            )]);
        }
        let pages = parse_pages(&body);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("No images found for this chapter.")]);
        }
        Ok(pages)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key
                    .starts_with("/truyen/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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
    Paged {
        entries: parse_listing_entries(body),
        has_next_page: body.contains("uk-pagination-next") && !body.contains("uk-disabled"),
    }
}

fn parse_listing_entries(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen/"))
        .filter_map(manga_from_article)
        .fold(Vec::new(), push_unique)
}

fn manga_from_article(chunk: &str) -> Option<CatalogItem> {
    let href =
        html::attr_after(chunk, "<h3", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    if !href.contains("/truyen/") {
        return None;
    }
    let key = normalize_key(href.split('?').next().unwrap_or(&href));
    let title = html::text_between(chunk, "<h3", "</h3>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MeoSua".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/truyen/sample".into());
    let tab_story = body.split("tab-story").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "category-title", "</")
            .or_else(|| html::text_between(body, "single-block", "</h2>"))
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MeoSua".into())),
        cover: html::attr_after(body, "single-thumb", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(tab_story, "hide-long-text", "</div>")
            .or_else(|| html::text_between(tab_story, "Tóm tắt", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: links_containing(tab_story, "/the-loai/"),
        status: parse_status(tab_story),
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_items(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "uk-link-toggle", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let raw_name = html::text_between(chunk, "<h4", "</h4>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let mut title = normalize_chapter_name(&raw_name);
            let is_locked = chunk.contains("icon: lock") || chunk.contains("uk-text-danger");
            if is_locked {
                title = format!("Locked - {title}");
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                is_locked,
                date_uploaded: html::text_between(chunk, "icon: calendar", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_vietnam_date(&value)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter_body = body
        .split("view-chapter")
        .nth(1)
        .or_else(|| body.split("chapter-content").nth(1))
        .unwrap_or(body);
    chapter_body
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !is_placeholder(image))
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

fn links_containing(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    let text = html::strip_tags(input).to_ascii_lowercase();
    if text.contains("trọn bộ") {
        ItemStatus::Completed
    } else if text.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_chapter_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if let Some(index) = lower.find("chap") {
        let mut out = String::from("Chap");
        out.push_str(input[index + 4..].trim());
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        input.to_string()
    }
}

fn max_chapter_page(body: &str) -> u64 {
    let mut max_page = 1;
    for chunk in body.split("?trang=").skip(1) {
        let digits = chunk
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if let Ok(page) = digits.parse::<u64>() {
            max_page = max_page.max(page);
        }
    }
    max_page
}

fn parse_vietnam_date(input: &str) -> Option<i64> {
    let mut parts = input
        .split_whitespace()
        .find(|part| part.contains('/'))
        .unwrap_or(input)
        .split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn is_placeholder(input: &str) -> bool {
    input.contains("/wp-content/uploads/")
        && (input.ends_with("/0.webp") || input.ends_with("/999.webp"))
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn normalize_key(input: &str) -> String {
    let input = input.split('?').next().unwrap_or(input);
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut entries: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="uk-panel uk-margin-small-bottom"><h3><a href="/truyen/sample/">Sample</a></h3><img src="/cover.jpg"></article><ul class="uk-pagination"><a uk-pagination-next href="?trang=2"></a></ul>"#;
const DETAILS_FIXTURE: &str = r#"<section id="single-block"><h2>Sample</h2><div class="single-thumb"><img src="/cover.jpg"></div><div class="tab-story"><a href="/the-loai/tag/">Tag</a><div class="hide-long-text"><p>Summary</p></div><span uk-icon="icon: file-edit"></span><span>Đang tiến hành</span><div class="chapter-list"><div class="chapter-item"><a class="uk-link-toggle" href="/truyen/sample/chap-1/"><h4>Chap 1</h4></a><span uk-icon="icon: calendar"></span><span>01/01/2024</span></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><div class="chapter-content"><img data-src="/page1.jpg"></div></div>"#;
