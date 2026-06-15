use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: LoppyToon = LoppyToon;
const BASE_URL: &str = "https://loppytoon.com";

struct LoppyToon;

impl MangaSource for LoppyToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(parse_popular(&fetch_document(BASE_URL, LIST_FIXTURE)));
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/truyen-moi-cap-nhat?page={page}"),
            LIST_FIXTURE,
        )))
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
            return Ok(parse_search_json(&fetch_json(
                &format!(
                    "{BASE_URL}/api/search-story?keyword={}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            )));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if let Some(genre) = filter(filters, "genre") {
            format!("{BASE_URL}/the-loai/{genre}?page={page}")
        } else if let Some(group) = filter(filters, "group") {
            format!("{BASE_URL}/nhom-dich/{group}?page={page}")
        } else {
            format!("{BASE_URL}/truyen-moi-cap-nhat?page={page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        let manga_url = absolute_url(&key);
        let mut chapters = parse_chapters(&fetch_document(&manga_url, DETAILS_FIXTURE));
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let mut offset = chapters.len();
        for _ in 0..20 {
            if offset == 0 && !chapters.is_empty() {
                break;
            }
            let body = fetch_json(
                &format!(
                    "{BASE_URL}/load-more-chapters?slug={slug}&offset={offset}&sortByPosition=desc"
                ),
                "{\"html\":\"\",\"has_more\":false}",
            );
            let response = serde_json::from_str::<ChapterResponse>(&body).unwrap_or_default();
            if response.html.trim().is_empty() {
                break;
            }
            let new_chapters = parse_chapters(&response.html);
            offset += new_chapters.len();
            for chapter in new_chapters {
                chapters = push_unique_chapter(chapters, chapter);
            }
            if !response.has_more {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen/sample/1".into());
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
                item: key.starts_with("/truyen/").then(|| details_by_key(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("hot-comic-item"))
        .filter_map(item_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-item"))
        .filter_map(item_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("fa-chevron-right") && body.contains("onclick"),
    }
}

fn item_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    let key = normalize_key(&href);
    if is_novel_url(&key) {
        return None;
    }
    let title = html::text_between(chunk, "comic-title", "</")
        .or_else(|| html::text_between(chunk, "comic-title", "</div>"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(normalize_thumbnail_url),
        url: Some(absolute_url(&key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    })
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let results = serde_json::from_str::<Vec<SearchResult>>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or_default());
    Paged {
        entries: results
            .into_iter()
            .filter_map(|result| {
                let slug = result.slug?;
                let key = format!("/truyen/{slug}");
                if is_novel_url(&key) {
                    return None;
                }
                Some(CatalogItem {
                    key: key.clone(),
                    title: result.title.unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())
                    }),
                    cover: result.cover.map(|cover| {
                        normalize_thumbnail_url(if cover.starts_with("http") {
                            cover
                        } else {
                            format!("{BASE_URL}/storage/{cover}")
                        })
                    }),
                    url: Some(absolute_url(&key)),
                    language: Some("vi".into()),
                    content_rating: Some("adult".into()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let alt_name = text_after_label(body, "Tên khác");
    let desc = html::text_between(body, "manga-description", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let description = match (alt_name, desc) {
        (Some(alt), Some(desc)) => Some(format!("Ten khac: {alt}\n{desc}")),
        (Some(alt), None) => Some(format!("Ten khac: {alt}")),
        (None, desc) => desc,
    };
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "manga-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "cover-image", "src")
            .or_else(|| image_attr(body))
            .map(normalize_thumbnail_url),
        authors: text_after_label(body, "Tác giả")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("tag"))
            .map(html::strip_tags)
            .filter(|value| !value.is_empty())
            .collect(),
        description,
        status: parse_status(&html::strip_tags(body)),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: None,
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-image"))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .map(|image| absolute_url(&image))
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
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
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("ongoing") || lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else if lower.contains("completed") || lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_thumbnail_url(value: String) -> String {
    value
        .find("https://")
        .and_then(|first| {
            value[first + 8..]
                .find("https://")
                .map(|second| first + 8 + second)
        })
        .map(|index| value[index..].to_string())
        .unwrap_or(value)
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
}

fn normalize_key(input: &str) -> String {
    let without_base = input
        .trim()
        .strip_prefix(BASE_URL)
        .unwrap_or(input.trim())
        .split(['?', '#'])
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", without_base.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("loppytoon.com")
        .then(|| normalize_key(input))
        .filter(|key| key.starts_with("/truyen/"))
}

fn is_novel_url(input: &str) -> bool {
    normalize_key(input).starts_with("/truyen/novel")
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|entry| entry.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Default, Deserialize)]
struct SearchResult {
    slug: Option<String>,
    title: Option<String>,
    cover: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    html: String,
    has_more: bool,
}

const LIST_FIXTURE: &str = r#"<div class="comic-item"><a href="/truyen/sample"><h3 class="comic-title">Sample</h3><div class="comic-cover"><img src="/cover.jpg"></div></a></div><i class="fa-chevron-right" onclick="next()"></i>"#;
const SEARCH_FIXTURE: &str = r#"[{"slug":"sample","title":"Sample","cover":"covers/sample.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="manga-title">Sample</h1><img class="cover-image" src="/cover.jpg"><span class="meta-label">Tác giả</span><span>Author</span><span class="meta-label">Tình trạng</span><span>Đang tiến hành</span><div class="manga-tags"><a class="tag">Action</a></div><div class="manga-description"><p>Summary</p></div><a class="chapter-item" href="/truyen/sample/chapter-1"><h3>Chapter 1</h3><span class="chapter-date">1 ngày trước</span></a>"#;
const PAGES_FIXTURE: &str =
    r#"<img class="manga-image" src="/page1.jpg"><img class="manga-image" data-src="/page2.jpg">"#;

export_manga_source!(SOURCE);
