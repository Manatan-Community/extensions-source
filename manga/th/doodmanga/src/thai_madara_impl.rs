use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, abi::WebViewRequest, abi::WebViewScript,
    abi::WebViewScriptRunAt, abi::WebViewWait, abi::webview_open, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

#[derive(Clone, Copy)]
struct ThaiMadaraConfig {
    base_url: &'static str,
    name: &'static str,
    lang: &'static str,
    content_rating: &'static str,
    manga_path: &'static str,
    latest_enabled: bool,
}

impl ThaiMadaraConfig {
    fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            let marker = format!("/{}/", self.manga_path.trim_matches('/'));
            if let Some(index) = value.find(&marker) {
                return format!("/{}", value[index + 1..].trim_end_matches('/'));
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    fn list_url(&self, page: u64, order: &str) -> String {
        let page_path = if page <= 1 {
            String::new()
        } else {
            format!("page/{page}/")
        };
        format!(
            "{}/{}/{}?m_orderby={}",
            self.base_url.trim_end_matches('/'),
            self.manga_path.trim_matches('/'),
            page_path,
            order
        )
    }

    fn search_url(&self, page: u64, query: &str) -> String {
        let page_path = if page <= 1 {
            String::new()
        } else {
            format!("page/{page}/")
        };
        format!(
            "{}/{}?s={}&post_type=wp-manga",
            self.base_url.trim_end_matches('/'),
            page_path,
            url::query_escape(query)
        )
    }
}

impl MangaSource for ThaiMadaraSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str);
        let order = if listing == Some("latest") && CONFIG.latest_enabled {
            "latest"
        } else {
            "views"
        };
        let body = if request.as_object().is_some_and(|object| object.is_empty()) {
            LIST_FIXTURE.to_string()
        } else {
            fetch_document_or_fixture(&CONFIG.list_url(page, order), LIST_FIXTURE)
        };
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(&CONFIG.search_url(page, query), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/sample", CONFIG.manga_path.trim_matches('/')));
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/sample", CONFIG.manga_path.trim_matches('/')));
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            format!(
                "/{}/sample/chapter-1",
                CONFIG.manga_path.trim_matches('/')
            )
        });
        let chapter_url = CONFIG.absolute_url(&key);
        let body = fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE);
        let mut pages = parse_pages(&body, &chapter_url);
        if pages.is_empty() && request.as_object().is_some_and(|object| !object.is_empty()) {
            pages = webview_pages(&chapter_url);
        }
        Ok(pages)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains(CONFIG.manga_path) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(CONFIG.normalize_key(input)),
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
        .with_referer(format!("{}/", CONFIG.base_url.trim_end_matches('/')))
        .with_cookies_for(CONFIG.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("page-item-detail")
                || chunk.contains("manga__item")
                || chunk.contains("c-tabs-item")
                || chunk.contains("post-title")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<h3", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains(CONFIG.manga_path) {
                return None;
            }
            let title = html::text_between(chunk, "post-title", "</a>")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| CONFIG.name.into()));
            let key = CONFIG.normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| CONFIG.absolute_url(&image)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some(CONFIG.lang.to_string()),
                content_rating: Some(CONFIG.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn has_next_page(body: &str) -> bool {
    body.contains("nav-previous") || body.contains("navigation-ajax") || body.contains("next page")
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
        .map(|value| CONFIG.normalize_key(&value))
        .unwrap_or_else(|| format!("/{}/sample", CONFIG.manga_path.trim_matches('/')));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| CONFIG.name.into())),
        cover: html::attr_after(body, "summary_image", "src")
            .or_else(|| html::attr_after(body, "tab-summary", "src"))
            .or_else(|| image_attr(body))
            .map(|image| CONFIG.absolute_url(&image)),
        description: html::text_between(body, "description-summary", "</div>")
            .or_else(|| html::text_between(body, "summary__content", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "author"),
        artists: info_values(body, "artist"),
        tags: info_values(body, "genres"),
        status: parse_status(body),
        url: Some(CONFIG.absolute_url(&key)),
        language: Some(CONFIG.lang.to_string()),
        content_rating: Some(CONFIG.content_rating.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                url: Some(CONFIG.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(CONFIG.absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let mut images = script_images(body);
    images.extend(body.split("<img").skip(1).filter_map(image_attr).filter(|image| {
        !image.starts_with("data:")
            && !image.is_empty()
            && !image.contains("logo")
            && !image.contains("avatar")
            && !image.contains("cover")
    }));
    images
        .into_iter()
        .fold(Vec::<String>::new(), |mut out, image| {
            let image = CONFIG.absolute_url(&image);
            if !out.contains(&image) {
                out.push(image);
            }
            out
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn webview_pages(chapter_url: &str) -> Vec<MangaPage> {
    let script = r#"
Array.from(document.querySelectorAll('.reading-content img,#readerarea img,div.text-center img'))
  .map((img) => img.currentSrc || img.dataset.src || img.dataset.lazySrc || img.src)
  .filter(Boolean)
"#;
    let response = webview_open(&WebViewRequest {
        url: chapter_url.to_string(),
        wait_for: Some(WebViewWait::Delay { milliseconds: 2500 }),
        scripts: vec![WebViewScript {
            id: Some("pages".to_string()),
            script: script.to_string(),
            run_at: Some(WebViewScriptRunAt::AfterWait),
        }],
        return_html: false,
        timeout_ms: Some(30_000),
        ..WebViewRequest::default()
    });
    let Ok(response) = response else {
        return Vec::new();
    };
    response
        .script_results
        .into_iter()
        .find(|result| result.id.as_deref() == Some("pages"))
        .and_then(|result| result.value)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: CONFIG.absolute_url(&image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
}

fn script_images(body: &str) -> Vec<String> {
    let mut images = Vec::new();
    if let Some(start) = body.find("\"images\"")
        && let Some(open) = body[start..].find('[').map(|index| start + index)
        && let Some(close) = body[open..].find(']').map(|index| open + index + 1)
    {
        images.extend(serde_json::from_str::<Vec<String>>(&body[open..close]).unwrap_or_default());
    }
    images
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") || lower.contains("จบ") {
        ItemStatus::Completed
    } else if lower.contains("dropped") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") || lower.contains("on going") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn info_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.to_ascii_lowercase().contains(marker))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect()
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
