use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: DocTruyen5s = DocTruyen5s;
const BASE_URL: &str = "https://manga.io.vn";

struct DocTruyen5s;

impl MangaSource for DocTruyen5s {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("/all-manga/{page}/?sort=last_update&status=0")
        } else {
            format!("/ranking/week/{page}")
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}{path}"),
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
        let target = if query.is_empty() {
            format!("{BASE_URL}/filter/{page}/")
        } else {
            format!(
                "{BASE_URL}/search/{page}/?keyword={}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
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
                item: key.contains("/manga/").then(|| details_by_key(&key)),
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

fn fetch_json(target: &str, referer: &str, fixture: &str) -> String {
    client()
        .get(target)
        .referer(referer)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("grid") || chunk.contains("text-center") || chunk.contains("manga")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, ".text-center a", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ".text-center", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
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
    Paged {
        entries,
        has_next_page: body.contains("blog-pager") || body.contains("pagecurrent"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, ".a2 header h1", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, ".a1", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: html::text_between(body, "fa-user", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: link_texts_by_href(body, "rel=\"tag\""),
        description: html::text_between(body, "syn-target", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(body)),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
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
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| value.parse::<i64>().ok())
                    .map(|seconds| seconds * 1000),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let mut content = body.to_string();
    if let Some(chapter_id) = body
        .split("const CHAPTER_ID = ")
        .nth(1)
        .and_then(|tail| tail.split(';').next())
    {
        let api = format!(
            "{BASE_URL}/ajax/image/list/chap/{}",
            chapter_id.trim().trim_matches('"').trim_matches('\'')
        );
        if let Ok(payload) = serde_json::from_str::<PageListResponse>(&fetch_json(
            &api,
            chapter_url,
            PAGES_API_FIXTURE,
        )) {
            if payload.status {
                content = payload.html;
            }
        }
    }
    content
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("separator"))
        .filter_map(|chunk| html::attr_after(chunk, "<a", "href"))
        .chain(body.split("<img").skip(1).filter_map(image_attr))
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
    if lower.contains("hoàn thành") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("đang") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-lazy-src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
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

fn link_texts_by_href(body: &str, href_marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_marker))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
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
        .filter(|key| key.contains("/manga/"))
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

#[derive(Deserialize)]
struct PageListResponse {
    status: bool,
    html: String,
}

const LIST_FIXTURE: &str = r#"<div class="grid"><div><img src="/cover.jpg"><div class="text-center"><a href="/manga/sample">Sample</a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="a1"><figure><img src="/cover.jpg"></figure></div><div class="a2"><header><h1>Sample</h1></header><a rel="tag">Action</a><div id="syn-target">Summary</div></div><ul><li class="chapter"><a href="/manga/sample/chapter-1">Chapter 1</a><time datetime="1704067200"></time></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<script>const CHAPTER_ID = 1;</script><div class="separator"><a href="/page1.jpg"></a></div>"#;
const PAGES_API_FIXTURE: &str =
    r#"{"status":true,"html":"<div class=\"separator\"><a href=\"/page1.jpg\"></a></div>"}"#;

export_manga_source!(SOURCE);
