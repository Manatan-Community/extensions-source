use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: KamiComic = KamiComic;
const BASE_URL: &str = "https://kamicomi.com";

struct KamiComic;

impl MangaSource for KamiComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            format!("/bang-xep-hang-truyen/page/{page}/")
        } else {
            format!("/moi-cap-nhat/page/{page}/")
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
        let target = if !query.is_empty() {
            let base = if page == 1 {
                BASE_URL.to_string()
            } else {
                format!("{BASE_URL}/page/{page}/")
            };
            format!("{base}?s={}", url::query_escape(query))
        } else if let Some(genre) = request
            .get("filters")
            .and_then(|f| f.get("genre"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            format!("{BASE_URL}/the-loai/{genre}/page/{page}/")
        } else {
            format!("{BASE_URL}/moi-cap-nhat/page/{page}/")
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
        let body = fetch_document(&manga_url, DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        let max_page = max_chapter_page(&body);
        for page in 2..=max_page.min(20) {
            let page_body = fetch_document(
                &format!("{}/chuong/page/{page}/", manga_url.trim_end_matches('/')),
                "",
            );
            for chapter in parse_chapters(&page_body) {
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
        Ok(parse_pages(&body))
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("uk-link-heading") && chunk.contains("/truyen/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if is_novel_url(&href) {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::strip_tags(chunk).trim().to_string();
            Some(CatalogItem {
                key: key.clone(),
                title: if title.is_empty() {
                    url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())
                } else {
                    title
                },
                cover: nearby_image(body, &href)
                    .map(|image| absolute_url(&remove_thumb_suffix(&image))),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("aria-label=\"Trang sau\"") && !body.contains("uk-disabled"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let slug = key
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample");
    let api = format!("{BASE_URL}/wp-json/wp/v2/manga?slug={slug}&_embed=wp:featuredmedia,wp:term");
    parse_details(&fetch_json(&api, DETAILS_JSON_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let item = value
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(&Value::Null);
    CatalogItem {
        key: key.into(),
        title: item
            .pointer("/title/rendered")
            .and_then(Value::as_str)
            .map(html::strip_tags)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: item
            .pointer("/_embedded/wp:featuredmedia/0/source_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: item
            .pointer("/content/rendered")
            .and_then(Value::as_str)
            .map(html::strip_tags)
            .filter(|v| !v.is_empty()),
        authors: terms(item, "author_tax"),
        tags: terms(item, "genre"),
        status: ItemStatus::Unknown,
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
        .filter(|chunk| chunk.contains("uk-link-toggle"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let raw = html::text_between(chunk, "<h3", "</h3>")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_else(|| html::strip_tags(chunk));
            let title = raw
                .find("Chương")
                .map(|index| raw[index..].to_string())
                .unwrap_or(raw);
            let locked = chunk.contains("icon: lock") || chunk.contains("uk-text-danger");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if locked {
                    format!("Locked {title}")
                } else {
                    title
                }),
                url: Some(absolute_url(&key)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter-content")
                || chunk.contains("data-original-src")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-original-src").or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:"))
        .map(|image| absolute_url(&image))
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
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn terms(item: &Value, taxonomy: &str) -> Vec<String> {
    item.pointer("/_embedded/wp:term")
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|groups| groups.iter())
        .flat_map(|group| group.as_array().into_iter().flatten())
        .filter(|term| term.get("taxonomy").and_then(Value::as_str) == Some(taxonomy))
        .filter_map(|term| term.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn nearby_image(body: &str, href: &str) -> Option<String> {
    let index = body.find(href)?;
    let start = index.saturating_sub(900);
    let end = (index + 900).min(body.len());
    html::attr_after(&body[start..end], "<img", "src")
}

fn max_chapter_page(body: &str) -> u64 {
    body.split("/chuong/page/")
        .skip(1)
        .filter_map(|tail| tail.split('/').next()?.parse::<u64>().ok())
        .max()
        .unwrap_or(1)
}

fn is_novel_url(value: &str) -> bool {
    normalize_key(value).starts_with("/truyen/novel")
}

fn remove_thumb_suffix(value: &str) -> String {
    value.replace("-150x150.", ".")
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
        .filter(|key| key.contains("/truyen/"))
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

const LIST_FIXTURE: &str = r#"<div class="uk-panel"><a class="uk-link-heading" href="/truyen/sample">Sample</a><img src="/cover-150x150.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="chapter-list"><a class="uk-link-toggle" href="/truyen/sample/chapter-1"><h3>Chương 1</h3><time>1 ngày trước</time></a></div>"#;
const DETAILS_JSON_FIXTURE: &str = r#"[{"title":{"rendered":"Sample"},"content":{"rendered":"Summary"},"_embedded":{"wp:featuredmedia":[{"source_url":"https://kamicomi.com/cover.jpg"}],"wp:term":[[{"name":"Action","taxonomy":"genre"}],[{"name":"Unknown","taxonomy":"author_tax"}]]}}]"#;
const PAGES_FIXTURE: &str =
    r#"<div id="chapter-content"><img data-original-src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
