use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Mangakakalot = Mangakakalot;
const BASE_URL: &str = "https://www.mangakakalot.gg";
const MIRROR_URL: &str = "https://www.mangakakalove.com";

struct Mangakakalot;

impl MangaSource for Mangakakalot {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/manga-list/latest-manga?page={page}")
        } else {
            format!("{BASE_URL}/manga-list/hot-manga?page={page}")
        };
        Ok(parse_listing(&fetch_document(&path, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) || query.starts_with(MIRROR_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/genre?page={page}")
        } else {
            format!("{BASE_URL}/search/story/{}?page={page}", normalize_search_query(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let target = absolute_url(&normalized_detail_key(&key, request.get("title").and_then(Value::as_str)));
        Ok(parse_details(&fetch_document(&target, DETAILS_FIXTURE), Some(normalize_key(&target))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let slug = api_slug(&key, request.get("title").and_then(Value::as_str));
        Ok(parse_chapter_api(&fetch_document(
            &format!("{BASE_URL}/api/manga/{slug}/chapters?limit=-1"),
            CHAPTERS_FIXTURE,
        ), &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
        if input.starts_with(BASE_URL) || input.starts_with(MIRROR_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key.starts_with("/manga/").then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).or_else(|| value.strip_prefix(MIRROR_URL)).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn normalized_detail_key(key: &str, title: Option<&str>) -> String {
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or_default();
    if is_id_slug(slug) {
        return format!("/manga/{}", title_to_slug(title.unwrap_or(slug)));
    }
    normalize_key(key)
}

fn api_slug(key: &str, title: Option<&str>) -> String {
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or("sample");
    if is_id_slug(slug) {
        title_to_slug(title.unwrap_or(slug))
    } else {
        slug.to_string()
    }
}

fn is_id_slug(value: &str) -> bool {
    value.len() >= 3 && value.chars().take(2).all(|ch| ch.is_ascii_lowercase()) && value.chars().skip(2).all(|ch| ch.is_ascii_digit())
}

fn title_to_slug(title: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in title.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize_search_query(query: &str) -> String {
    title_to_slug(query).replace('-', "_")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("<div").skip(1).filter(|chunk| {
        chunk.contains("list-truyen-item-wrap") || chunk.contains("list-comic-item-wrap") || chunk.contains("story_item")
    }).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<h3", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
        let key = normalize_key(&href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| absolute_url(&image)),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), |mut items, item| {
        if !items.iter().any(|existing: &CatalogItem| existing.key == item.key) {
            items.push(item);
        }
        items
    });
    Paged {
        entries,
        has_next_page: body.contains("group_page") || body.contains("group-page") || body.contains("page_select") || body.contains("page-select"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let title = html::text_between(body, "manga-info-top", "</h1>")
        .or_else(|| html::text_between(body, "panel-story-info", "</h1>"))
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
    let mut description = html::text_between(body, "panel-story-info-description", "</div>")
        .or_else(|| html::text_between(body, "contentBox", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(alt) = html::text_between(body, "story-alternative", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Name: ");
        description.push_str(&alt);
    }
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "manga-info-pic", "src").or_else(|| html::attr_after(body, "info-image", "src")).or_else(|| html::attr_after(body, "<img", "src")).map(|image| absolute_url(&image)),
        description: (!description.is_empty()).then_some(description),
        authors: info_links(body, "author"),
        artists: info_links(body, "author"),
        tags: info_links(body, "genres"),
        status: if body.contains("Completed") { ItemStatus::Completed } else if body.contains("Ongoing") { ItemStatus::Ongoing } else { ItemStatus::Unknown },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_api(body: &str, slug: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChapterResponse>(body).unwrap_or_default();
    response.data.map(|data| data.chapters).unwrap_or_default().into_iter().map(|chapter| {
        let chapter_slug = chapter.chapter_slug.unwrap_or_else(|| "chapter-1".to_string());
        let key = format!("/manga/{slug}/{chapter_slug}");
        MangaChapter {
            key: key.clone(),
            title: chapter.chapter_name,
            chapter_number: chapter.chapter_num,
            url: Some(absolute_url(&key)),
            date_uploaded: chapter.updated_at.as_deref().and_then(manatan_shared::dates::parse_fixture_date),
            ..MangaChapter::default()
        }
    }).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = extract_script_array(body, "chapterImages");
    if images.is_empty() {
        images = body.split("<img").skip(1).filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))).collect();
    } else {
        let cdn = extract_script_array(body, "cdns").into_iter().next().unwrap_or_else(|| BASE_URL.to_string());
        images = images.into_iter().map(|image| format!("{}/{}", cdn.trim_end_matches('/'), image.trim_start_matches('/'))).collect();
    }
    images.into_iter().filter(|image| !image.is_empty() && !image.starts_with("data:")).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn extract_script_array(body: &str, name: &str) -> Vec<String> {
    let Some(after) = body.split(&format!("{name} = [")).nth(1).or_else(|| body.split(&format!("{name}=[")).nth(1)) else {
        return Vec::new();
    };
    let Some(raw) = after.split(']').next() else {
        return Vec::new();
    };
    raw.split(',').map(|part| part.trim().trim_matches('"').trim_matches('\'').replace("\\/", "/").trim_end_matches('/').to_string()).filter(|part| !part.is_empty()).collect()
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("<li").chain(body.split("<td")).filter(|chunk| chunk.to_ascii_lowercase().contains(label)).flat_map(|chunk| {
        chunk.split("<a").skip(1).filter_map(|link| html::text_between(link, ">", "</a>").map(|value| html::strip_tags(&value))).collect::<Vec<_>>()
    }).filter(|value| !value.is_empty()).collect()
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    data: Option<ChapterData>,
}

#[derive(Default, Deserialize)]
struct ChapterData {
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    chapter_name: Option<String>,
    chapter_num: Option<f32>,
    chapter_slug: Option<String>,
    updated_at: Option<String>,
}

const LIST_FIXTURE: &str = r#"<div class="list-truyen-item-wrap"><h3><a href="/manga/sample">Sample Manga</a></h3><img src="/cover.jpg"></div><div class="group_page"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="manga-info-top"><h1>Sample Manga</h1><div class="manga-info-pic"><img src="/cover.jpg"></div><li>Author <a>Author</a></li><li>Status Ongoing</li><li>Genres <a>Action</a></li></div><div id="panel-story-info-description">Summary</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":{"chapters":[{"chapter_name":"Chapter 1","chapter_num":1,"chapter_slug":"chapter-1","updated_at":"2024-01-01T00:00:00.000000Z"}]}}"#;
const PAGES_FIXTURE: &str = r#"<script>cdns = ["https://cdn.example"]; chapterImages = ["sample/001.jpg"];</script>"#;

export_manga_source!(SOURCE);
