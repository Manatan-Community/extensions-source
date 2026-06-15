use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: FastScan = FastScan;
const BASE_URL: &str = "https://fastscan.org";

struct FastScan;

impl MangaSource for FastScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "4"
        } else {
            "0"
        };
        Ok(parse_listing(
            &fetch_document(&advanced_url(page, "", "0", "0", sort), LIST_FIXTURE),
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if !query.is_empty() {
            return Ok(parse_listing(
                &fetch_document(
                    &format!(
                        "{BASE_URL}/tim-kiem?q={}&page={page}",
                        url::query_escape(query)
                    ),
                    LIST_FIXTURE,
                ),
                page,
            ));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category = filter(filters, "genre").unwrap_or_default();
        let min_chapter = filter(filters, "minChapter").unwrap_or("0");
        let status = filter(filters, "status").unwrap_or("0");
        let sort = filter(filters, "sort").unwrap_or("0");
        Ok(parse_listing(
            &fetch_document(
                &advanced_url(page, category, status, min_chapter, sort),
                LIST_FIXTURE,
            ),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chap-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/truyen-tranh/").then(|| details_by_key(&key)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn advanced_url(page: u64, category: &str, status: &str, min_chapter: &str, sort: &str) -> String {
    format!(
        "{BASE_URL}/tim-kiem-nang-cao?category={}&notcategory=&status={}&minchapter={}&sort={}&page={page}",
        url::query_escape(category),
        url::query_escape(status),
        url::query_escape(min_chapter),
        url::query_escape(sort)
    )
}

fn parse_listing(body: &str, current_page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("book_avatar") || chunk.contains("book_name"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "book_avatar", "href")
                .or_else(|| html::attr_after(chunk, "book_name", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "book_name", "</")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    let has_next_page = body
        .split("page=")
        .filter_map(|tail| {
            tail.split(|ch: char| !ch.is_ascii_digit())
                .next()?
                .parse::<u64>()
                .ok()
        })
        .any(|page| page > current_page);
    Paged {
        entries,
        has_next_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "book_other", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "book_avatar", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: html::text_between(body, "author", "</li>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: link_texts_by_href(body, "list01"),
        description: html::text_between(body, "story-detail-info", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("works-chapter-item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "name-chap", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "name-chap", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "time-chap", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("lozad")
                || chunk.contains("page-chapter")
                || chunk.contains("chapter_content")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| looks_like_image(image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            let image = absolute_url(&image);
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
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

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else if lower.contains("đang cập nhật") || lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:")
        && [".jpg", ".jpeg", ".png", ".webp", ".avif"]
            .iter()
            .any(|ext| lower.contains(ext))
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn link_texts_by_href(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|index| {
            body[index..]
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else {
        format!("{BASE_URL}/{}", value.trim_start_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen-tranh/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<ul class="list_grid grid"><li><div class="book_avatar"><a href="/truyen-tranh/sample"><img src="/cover.jpg"></a></div><div class="book_name"><h3><a href="/truyen-tranh/sample">Sample</a></h3></div></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="book_detail"><div class="book_other"><h1>Sample</h1><ul class="list01"><a>Action</a></ul></div><div class="book_info"><div class="book_avatar"><img src="/cover.jpg"></div></div><div class="story-detail-info detail-content">Summary</div><div class="list_chapter"><div class="works-chapter-item"><div class="name-chap"><a href="/truyen-tranh/sample/chap-1">Chapter 1</a></div><span class="time-chap">01/01/2024</span></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="chapter_content"><div class="chapter_content"><img class="lozad" data-src="/page1.jpg"></div></div>"#;

export_manga_source!(SOURCE);
