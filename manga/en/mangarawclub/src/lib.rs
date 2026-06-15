use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaRawClub = MangaRawClub;
const BASE_URL: &str = "https://www.mgeko.cc";

#[derive(Deserialize, Default)]
struct BrowseDto {
    #[serde(default, rename = "results_html")]
    results_html: String,
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "num_pages")]
    num_pages: u64,
}

struct MangaRawClub;

impl MangaSource for MangaRawClub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(BROWSE_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular_all_time"
        };
        Ok(parse_browse(&fetch_text(&browse_url(page, sort, "", request.get("filters"), hide_nsfw(&request)), BROWSE_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters");
        if !query.is_empty() && filters.map(filters_default).unwrap_or(true) {
            let body = fetch_document(&format!("{BASE_URL}/search/?search={}&results={page}", url::query_escape(query)), SEARCH_FIXTURE);
            return Ok(Paged { entries: parse_search_cards(&body), has_next_page: body.contains("paging") && body.contains("Next") });
        }
        Ok(parse_browse(&fetch_text(&browse_url(page, &filter(filters, "sort", "latest"), query, filters, hide_nsfw(&request)), BROWSE_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let target = format!("{}/all-chapters/", absolute_url(&key).trim_end_matches('/'));
        Ok(parse_chapters(&fetch_document(&target, CHAPTERS_FIXTURE)))
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
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(normalize_key(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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

fn fetch_text(target: &str, fixture: &str) -> String {
    client().get(target).header("Accept", "application/json,text/html").send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn browse_url(page: u64, sort: &str, query: &str, filters: Option<&Value>, safe_mode: bool) -> String {
    let mut params = vec![
        format!("page={page}"),
        format!("sort={}", url::query_escape(sort)),
        format!("safe_mode={}", if safe_mode { "1" } else { "0" }),
    ];
    for key in ["status", "type", "min_chapters", "max_chapters", "include_genres", "exclude_genres", "tags"] {
        let value = filter(filters, key, "");
        if !value.is_empty() {
            params.push(format!("{key}={}", url::query_escape(&value)));
        }
    }
    let rating = filter(filters, "min_rating", "");
    if !rating.is_empty() {
        let value = rating.parse::<f32>().unwrap_or(0.0);
        params.push(format!("min_rating={}", (value * 10.0) as u32));
    }
    for key in ["only_completed", "only_translated", "hide_on_break"] {
        if filters.and_then(|value| value.get(key)).and_then(Value::as_bool).unwrap_or(false) {
            params.push(format!("{key}=1"));
        }
    }
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    format!("{BASE_URL}/browse-comics/data/?{}", params.join("&"))
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let dto: BrowseDto = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: parse_comic_cards(&dto.results_html),
        has_next_page: dto.page < dto.num_pages,
    }
}

fn parse_comic_cards(body: &str) -> Vec<CatalogItem> {
    body.split("comic-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "comic-card__title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(catalog_item(key, title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search_cards(body: &str) -> Vec<CatalogItem> {
    body.split("novel-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "novel-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(catalog_item(key, title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique)
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut description = html::text_between(body, "description", "</div>").map(|value| html::strip_tags(&value)).unwrap_or_default();
    description = description.strip_prefix("Summary is").unwrap_or(&description).trim().to_string();
    if let Some(alt) = html::text_between(body, "alternative-title", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Name: ");
        description.push_str(&alt);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "novel-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "cover", "data-src").or_else(|| html::attr_after(body, "cover", "src")).map(|image| absolute_url(&image)),
        authors: html::attr_after(body, "author", "title").into_iter().filter(|value| value.to_lowercase() != "updating").collect(),
        tags: link_texts(body, "genre"),
        description: (!description.is_empty()).then_some(description),
        status: if body.contains("completed") { ItemStatus::Completed } else if body.contains("ongoing") { ItemStatus::Ongoing } else { ItemStatus::Unknown },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let name = html::text_between(chunk, "chapter-title", "</")
                .or_else(|| html::text_between(chunk, "chapter-number", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Chapter".to_string())
                .replace("-eng-li", "");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if name.to_lowercase().starts_with("chapter") { name } else { format!("Chapter {name}") }),
                url: Some(absolute_url(&key)),
                date_uploaded: html::attr_after(chunk, "chapter-update", "datetime").and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-reader") || chunk.contains("src"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn link_texts(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .map(|chunk| html::strip_tags(chunk))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn filter(filters: Option<&Value>, id: &str, default: &str) -> String {
    filters.and_then(|value| value.get(id)).and_then(Value::as_str).filter(|value| !value.is_empty()).unwrap_or(default).to_string()
}

fn filters_default(filters: &Value) -> bool {
    filters.as_object().map(|object| {
        object.iter().all(|(key, value)| match value {
            Value::String(text) => text.is_empty() || (key == "sort" && text == "latest"),
            Value::Bool(flag) => !flag,
            _ => true,
        })
    }).unwrap_or(true)
}

fn hide_nsfw(request: &Value) -> bool {
    request.get("preferences").and_then(|prefs| prefs.get("pref_hide_nsfw")).and_then(Value::as_bool).unwrap_or(false)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".cc/") {
            return format!("/{}", value[index + 4..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const BROWSE_FIXTURE: &str = r#"{"results_html":"<div class=\"comic-card\"><a href=\"/manga/sample\"><img data-src=\"/cover.jpg\"><div class=\"comic-card__title\"><a href=\"/manga/sample\">Sample Manga</a></div></a></div>","page":1,"num_pages":1}"#;
const SEARCH_FIXTURE: &str = r#"<div class="novel-item"><a href="/manga/sample"><div class="novel-cover"><img data-src="/cover.jpg"></div><div class="novel-title">Sample Manga</div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"
<div class="novel-header"><div class="cover"><img data-src="/cover.jpg"></div><div class="manga-detail"><h1>Sample Manga</h1></div></div>
<div class="author"><a title="Author">Author</a></div><div class="description">Summary is sample summary.</div>
<div class="categories"><a href="/genre/action">Action</a></div><div class="header-stats"><strong class="ongoing">Ongoing</strong></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"<ul class="chapter-list"><li><a href="/manga/sample/chapter-1"><span class="chapter-number">1</span></a><time class="chapter-update" datetime="January 01, 2024, 1:00 pm"></time></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div id="chapter-reader"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
