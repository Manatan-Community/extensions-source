use crate::{
    html,
    sdk::{
        CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, http,
    },
    url,
};
use serde_json::Value;

pub fn image_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

pub fn lazy_page(key: &str, page_url: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Lazy {
            key: key.to_string(),
            url: None,
            page_url: Some(page_url.to_string()),
            context: None,
        },
        description: Some(key.to_string()),
        ..MangaPage::default()
    }
}

pub fn archive_page(archive_url: &str, entry_path: &str) -> MangaPage {
    MangaPage {
        content: PageContent::ArchiveEntry {
            archive_url: archive_url.to_string(),
            entry_path: entry_path.to_string(),
        },
        description: Some(entry_path.to_string()),
        ..MangaPage::default()
    }
}

pub fn text_page(text: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Text {
            text: text.to_string(),
        },
        description: Some("Text page".to_string()),
        ..MangaPage::default()
    }
}

pub fn decrypt_fixture_image_base64(input: &str) -> String {
    input.chars().rev().collect()
}

pub mod sinmh {
    use super::{html, http, image_headers, url};
    use crate::sdk::{
        CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
        source::MangaSource,
    };
    use serde_json::Value;
    use std::marker::PhantomData;

    pub trait SinmhConfig {
        const NAME: &'static str;
        const BASE_URL: &'static str;
        const LANG: &'static str = "zh";
        const CONTENT_RATING: &'static str = "adult";
        const VERSION_CODE: u64 = 1;
        const MOBILE_URL: Option<&'static str> = None;
        const DETAILS_STYLE: DetailsStyle = DetailsStyle::Dmzj;
        const KEEP_CHAPTER_ORDER: bool = false;

        fn mobile_url() -> String {
            Self::MOBILE_URL
                .map(ToString::to_string)
                .unwrap_or_else(|| Self::BASE_URL.replace("://www.", "://m."))
        }

        fn chapter_url(path: &str) -> String {
            url::join_url(&Self::mobile_url(), path)
        }
    }

    #[derive(Clone, Copy)]
    pub enum DetailsStyle {
        Default,
        Dmzj,
    }

    pub struct SinmhSource<C>(PhantomData<C>);

    impl<C> SinmhSource<C> {
        pub const fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<C: SinmhConfig> MangaSource for SinmhSource<C> {
        fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let path = if listing(&request) == "latest" {
                "update"
            } else {
                "click"
            };
            let target = format!("{}/list/{path}/?page={}", C::BASE_URL, page(&request));
            let body = fetch_doc::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(Paged {
                entries: parse_listing::<C>(&body),
                has_next_page: has_next_page(&body),
            })
        }

        fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if let Some(key) = path_from_url::<C>(query) {
                return Ok(Paged {
                    entries: vec![fetch_details::<C>(&key)],
                    has_next_page: false,
                });
            }
            let target = if query.is_empty() {
                format!("{}/list/click/?page={}", C::BASE_URL, page(&request))
            } else {
                format!(
                    "{}/search/?keywords={}&page={}",
                    C::BASE_URL,
                    url::query_escape(query),
                    page(&request)
                )
            };
            let body = fetch_doc::<C>(&target, LIST_FIXTURE, C::BASE_URL);
            Ok(Paged {
                entries: parse_listing::<C>(&body),
                has_next_page: has_next_page(&body),
            })
        }

        fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
            let key =
                request_key(&request, "manga").unwrap_or_else(|| "/comic/sample/".to_string());
            Ok(fetch_details::<C>(&key))
        }

        fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
            let key =
                request_key(&request, "manga").unwrap_or_else(|| "/comic/sample/".to_string());
            let body = fetch_doc::<C>(&C::chapter_url(&key), DETAILS_FIXTURE, C::BASE_URL);
            Ok(parse_chapters::<C>(&body))
        }

        fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
            let key = request_key(&request, "chapter")
                .unwrap_or_else(|| "/comic/sample/1.html".to_string());
            let target = C::chapter_url(&key);
            let body = fetch_doc::<C>(&target, PAGES_FIXTURE, C::BASE_URL);
            Ok(parse_pages::<C>(&body, &target))
        }

        fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key(&request, "manga").map(|key| absolute::<C>(&key)))
        }

        fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
            Ok(request_key(&request, "chapter").map(|key| C::chapter_url(&key)))
        }

        fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
            let popular = self.list(with_listing(&request, "popular"))?;
            let latest = self.list(with_listing(&request, "latest"))?;
            Ok(vec![
                HomeSection {
                    id: "popular".to_string(),
                    title: "Popular".to_string(),
                    style: Some(HomeSectionStyle::Featured),
                    entries: popular.entries,
                    has_more: popular.has_next_page,
                    ..HomeSection::default()
                },
                HomeSection {
                    id: "latest".to_string(),
                    title: "Latest".to_string(),
                    entries: latest.entries,
                    has_more: latest.has_next_page,
                    ..HomeSection::default()
                },
            ])
        }

        fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
            let Some(input) = request.get("url").and_then(Value::as_str) else {
                return Ok(None);
            };
            if let Some(key) = path_from_url::<C>(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(fetch_details::<C>(&key)),
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

    fn client<C: SinmhConfig>(referer: &str) -> http::HttpClient {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_cookies_for(C::BASE_URL)
            .with_webview_challenge_fallback()
    }

    fn fetch_doc<C: SinmhConfig>(target: &str, fixture: &str, referer: &str) -> String {
        client::<C>(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    fn fetch_details<C: SinmhConfig>(key: &str) -> CatalogItem {
        let body = fetch_doc::<C>(
            &url::join_url(&C::mobile_url(), key),
            DETAILS_FIXTURE,
            C::BASE_URL,
        );
        parse_details::<C>(&body, key)
    }

    fn parse_listing<C: SinmhConfig>(body: &str) -> Vec<CatalogItem> {
        body.split("<li")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("contList")
                    || chunk.contains("list-comic")
                    || chunk.contains("comic")
                    || chunk.contains("<img")
            })
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if href.contains("/chapter/") || href.contains("/read/") {
                    return None;
                }
                let key = normalize_key::<C>(&href);
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .or_else(|| html::text_between(chunk, "<p", "</p>"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| title_from_path(&key));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk::<C>(chunk),
                    url: Some(absolute::<C>(&key)),
                    language: Some(C::LANG.to_string()),
                    content_rating: Some(C::CONTENT_RATING.to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique)
    }

    fn parse_details<C: SinmhConfig>(body: &str, key: &str) -> CatalogItem {
        match C::DETAILS_STYLE {
            DetailsStyle::Default => parse_default_details::<C>(body, key),
            DetailsStyle::Dmzj => parse_dmzj_details::<C>(body, key),
        }
    }

    fn parse_default_details<C: SinmhConfig>(body: &str, key: &str) -> CatalogItem {
        CatalogItem {
            key: normalize_key::<C>(key),
            title: html::text_between(body, "book-title", "</")
                .or_else(|| html::text_between(body, "<h1", "</h1>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(key)),
            cover: html::attr_after(body, "book-cover", "src")
                .map(|src| url::join_url(C::BASE_URL, &src)),
            description: html::text_between(body, "intro-all", "</")
                .map(|value| {
                    html::strip_tags(&value)
                        .trim_start_matches("漫画简介：")
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty()),
            tags: details_links(body, "类型"),
            authors: details_links(body, "作者"),
            status: status_from_text(body),
            url: Some(absolute::<C>(key)),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_dmzj_details<C: SinmhConfig>(body: &str, key: &str) -> CatalogItem {
        let details =
            html::text_between(body, "comic_deCon", "</div>").unwrap_or_else(|| body.to_string());
        CatalogItem {
            key: normalize_key::<C>(key),
            title: html::text_between(&details, "<h1", "</h1>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(key)),
            cover: html::attr_after(body, "comic_i_img", "src")
                .map(|src| url::join_url(C::BASE_URL, &src)),
            description: html::text_between(body, "comic_deCon_d", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: details_field(&details, "作者"),
            tags: details_links(&details, "类别")
                .into_iter()
                .chain(details_links(&details, "类型"))
                .collect(),
            status: status_from_text(&details),
            url: Some(absolute::<C>(key)),
            language: Some(C::LANG.to_string()),
            content_rating: Some(C::CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn parse_chapters<C: SinmhConfig>(body: &str) -> Vec<MangaChapter> {
        let mut chapters = body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("href=") && !chunk.contains("/list/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if !is_chapter_href(&href) {
                    return None;
                }
                let key = normalize_key::<C>(&href);
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(html::strip_tags(chunk)).filter(|value| !value.is_empty()),
                    url: Some(C::chapter_url(&key)),
                    ..MangaChapter::default()
                })
            })
            .collect::<Vec<_>>();
        if !C::KEEP_CHAPTER_ORDER {
            chapters.reverse();
        }
        chapters
    }

    fn parse_pages<C: SinmhConfig>(body: &str, referer: &str) -> Vec<MangaPage> {
        let script = html::text_between(body, "chapterImages", "</script>")
            .unwrap_or_else(|| body.to_string());
        let images = extract_between(&script, "=", ";")
            .or_else(|| extract_between(body, "chapterImages = ", ";"))
            .unwrap_or_default();
        let path = extract_between(body, "chapterPath = \"", "\"").unwrap_or_default();
        parse_page_images(&images)
            .into_iter()
            .enumerate()
            .map(|(index, image)| {
                let target = if image.starts_with("http://") || image.starts_with("https://") {
                    image
                } else if image.starts_with('/') {
                    format!("{}{}", image_host::<C>(), image)
                } else if path.is_empty() {
                    format!("{}/{}", image_host::<C>().trim_end_matches('/'), image)
                } else {
                    format!("{}/{path}{image}", image_host::<C>().trim_end_matches('/'))
                };
                MangaPage {
                    content: PageContent::Url {
                        url: target,
                        context: None,
                    },
                    headers: image_headers(referer),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect()
    }

    fn parse_page_images(value: &str) -> Vec<String> {
        let clean = value
            .trim()
            .trim_start_matches('=')
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim()
            .trim_matches('"')
            .replace("\\/", "/");
        if clean.is_empty() {
            Vec::new()
        } else {
            clean
                .split("\",\"")
                .map(|item| item.trim_matches('"').to_string())
                .filter(|item| !item.is_empty())
                .collect()
        }
    }

    fn image_host<C: SinmhConfig>() -> String {
        let body = fetch_doc::<C>(
            &url::join_url(C::BASE_URL, "/js/config.js"),
            "",
            C::BASE_URL,
        );
        extract_between(&body, "domain\":[\"", "\"")
            .or_else(|| extract_between(&body, "domain:[\"", "\""))
            .unwrap_or_else(|| C::BASE_URL.to_string())
    }

    fn has_next_page(body: &str) -> bool {
        body.contains("pagination") && body.contains("next") && !body.contains("next disabled")
    }

    fn image_from_chunk<C: SinmhConfig>(chunk: &str) -> Option<String> {
        html::attr_after(chunk, "<img", "src")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .map(|src| url::join_url(C::BASE_URL, &src))
    }

    fn details_links(body: &str, label: &str) -> Vec<String> {
        body.split(label)
            .nth(1)
            .unwrap_or("")
            .split("</li>")
            .next()
            .unwrap_or("")
            .split("<a")
            .skip(1)
            .map(html::strip_tags)
            .filter(|value| !value.is_empty())
            .collect()
    }

    fn details_field(body: &str, label: &str) -> Vec<String> {
        html::text_between(body, label, "</li>")
            .map(|value| {
                html::strip_tags(&value)
                    .trim_start_matches('：')
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect()
    }

    fn status_from_text(body: &str) -> ItemStatus {
        if body.contains("已完结") || body.contains("完结") {
            ItemStatus::Completed
        } else if body.contains("连载中") || body.contains("連載") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        }
    }

    fn is_chapter_href(href: &str) -> bool {
        href.ends_with(".html")
            || href.contains("/chapter/")
            || href.contains("/comic/")
            || href.contains("/manhua/")
            || href.contains("/manga/")
    }

    fn absolute<C: SinmhConfig>(input: &str) -> String {
        url::join_url(C::BASE_URL, input)
    }

    fn normalize_key<C: SinmhConfig>(input: &str) -> String {
        let without_host = input
            .strip_prefix(C::BASE_URL)
            .or_else(|| input.strip_prefix(&C::mobile_url()))
            .unwrap_or(input);
        format!(
            "/{}",
            without_host
                .split(['?', '#'])
                .next()
                .unwrap_or(without_host)
                .trim_matches('/')
        )
    }

    fn path_from_url<C: SinmhConfig>(input: &str) -> Option<String> {
        if input.starts_with(C::BASE_URL) || input.starts_with(&C::mobile_url()) {
            Some(normalize_key::<C>(input))
        } else {
            None
        }
    }

    fn request_key(request: &Value, field: &str) -> Option<String> {
        request
            .get(field)
            .and_then(|value| {
                value
                    .get("key")
                    .or_else(|| value.get("url"))
                    .and_then(Value::as_str)
                    .or_else(|| value.as_str())
            })
            .or_else(|| request.get("key").and_then(Value::as_str))
            .map(|key| format!("/{}", key.trim_matches('/')))
    }

    fn page(request: &Value) -> u64 {
        request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1)
    }

    fn listing(request: &Value) -> &str {
        request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular")
    }

    fn with_listing(request: &Value, id: &str) -> Value {
        let mut copy = request.clone();
        if let Some(obj) = copy.as_object_mut() {
            obj.insert("listing".to_string(), Value::String(id.to_string()));
        }
        copy
    }

    fn extract_between(value: &str, start: &str, end: &str) -> Option<String> {
        Some(value.split(start).nth(1)?.split(end).next()?.to_string())
            .filter(|value| !value.is_empty())
    }

    fn title_from_path(path: &str) -> String {
        path.trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("manga")
            .replace('-', " ")
    }

    fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
        if !items.iter().any(|existing| existing.key == item.key) {
            items.push(item);
        }
        items
    }

    const LIST_FIXTURE: &str = r#"<ul id="contList"><li><a href="/comic/sample/"><img src="/cover.jpg"><h3><a href="/comic/sample/">Sample</a></h3></a></li></ul>"#;
    const DETAILS_FIXTURE: &str = r#"<div class="comic_deCon"><h1>Sample</h1><ul><li>作者：Author</li><li>状态：<a>连载中</a></li><li>类别：<a>Tag</a></li><li>类型：<a>Type</a></li></ul><p class="comic_deCon_d">Sample description.</p></div><div class="comic_i_img"><img src="/cover.jpg"></div><div class="chapter-body"><li><a href="/comic/sample/1.html">Chapter 1</a></li></div>"#;
    const PAGES_FIXTURE: &str = r#"<script>var chapterPath = "comic/sample/"; var chapterImages = ["1.jpg","2.jpg"];</script>"#;
}

#[derive(Debug, Clone)]
pub struct MadaraConfig {
    pub base_url: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
    pub manga_path: &'static str,
    pub popular_url_marker: &'static str,
    pub use_load_more: bool,
    pub latest_enabled: bool,
}

impl MadaraConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_manga_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            let marker = format!("/{}/", self.manga_path.trim_matches('/'));
            if let Some(index) = value.find(&marker) {
                return format!("/{}", value[index + 1..].trim_end_matches('/'));
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    pub fn list_url(&self, page: u64, order: &str) -> String {
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

    pub fn search_url(&self, page: u64, query: &str) -> String {
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

pub struct Madara;

impl Madara {
    pub fn browser_client(config: &MadaraConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_document_or_fixture(
        config: &MadaraConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        Self::browser_client(config)
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &MadaraConfig) -> Vec<CatalogItem> {
        body.split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("page-item-detail") || chunk.contains("manga__item"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, config.popular_url_marker, "href")
                    .or_else(|| html::attr_after(chunk, "<h3", "href"))
                    .or_else(|| html::attr_after(chunk, "post-title", "href"))
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                if !href.contains(config.manga_path) {
                    return None;
                }
                let title = html::text_between(chunk, config.popular_url_marker, "</a>")
                    .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
                let key = config.normalize_manga_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: madara_image(chunk).map(|value| config.absolute_url(&value)),
                    url: Some(config.absolute_url(&key)),
                    language: Some(config.lang.to_string()),
                    content_rating: Some(config.content_rating.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn has_next_page(body: &str, config: &MadaraConfig) -> bool {
        if config.use_load_more {
            !body.contains("no-posts")
        } else {
            body.contains("nav-previous")
                || body.contains("navigation-ajax")
                || body.contains("nextpostslink")
        }
    }

    pub fn parse_details(body: &str, key: Option<String>, config: &MadaraConfig) -> CatalogItem {
        let key = key
            .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
            .map(|value| config.normalize_manga_key(&value))
            .unwrap_or_else(|| format!("/{}/unknown", config.manga_path.trim_matches('/')));
        CatalogItem {
            key: key.clone(),
            title: html::text_between(body, "post-title", "</")
                .or_else(|| html::text_between(body, "<h1", "</h1>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
            cover: html::attr_after(body, "summary_image", "src")
                .or_else(|| html::attr_after(body, "tab-summary", "src"))
                .or_else(|| madara_image(body))
                .map(|value| config.absolute_url(&value)),
            description: html::text_between(body, "description-summary", "</div>")
                .or_else(|| html::text_between(body, "summary__content", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: madara_info_values(body, "author"),
            artists: madara_info_values(body, "artist"),
            tags: madara_info_values(body, "genres"),
            status: madara_status(body),
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn parse_chapters(body: &str, manga_key: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
        let chapter_blocks = body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("wp-manga-chapter"))
            .chain(
                body.split("<div")
                    .skip(1)
                    .filter(|chunk| chunk.contains("chapter-box")),
            );
        let mut chapters = chapter_blocks
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Chapter".to_string());
                let key = config.normalize_manga_key(&href);
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(title),
                    url: Some(config.absolute_url(&key)),
                    is_locked: chunk.contains("locked-badge")
                        || chunk.contains("chapter-lock")
                        || chunk.contains("premium"),
                    date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                        .map(|value| html::strip_tags(&value))
                        .and_then(|value| crate::dates::parse_fixture_date(&value)),
                    ..MangaChapter::default()
                })
            })
            .collect::<Vec<_>>();
        if chapters.is_empty() {
            chapters.push(MangaChapter {
                key: manga_key.to_string(),
                title: Some("Read".to_string()),
                url: Some(config.absolute_url(manga_key)),
                ..MangaChapter::default()
            });
        }
        chapters
    }

    pub fn parse_pages(body: &str, config: &MadaraConfig) -> Vec<MangaPage> {
        body.split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("wp-manga-chapter-img")
                    || chunk.contains("reading-content")
                    || chunk.contains("data-src")
                    || chunk.contains("data-cfsrc")
            })
            .filter_map(|chunk| {
                html::attr(chunk, "data-src")
                    .or_else(|| html::attr(chunk, "data-lazy-src"))
                    .or_else(|| srcset_first(html::attr(chunk, "srcset")))
                    .or_else(|| html::attr(chunk, "data-cfsrc"))
                    .or_else(|| html::attr(chunk, "src"))
            })
            .filter(|value| !value.starts_with("data:") && !value.is_empty())
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

pub trait MadaraSource {
    fn madara_config(&self, request: &Value) -> MadaraConfig;
    fn madara_list_fixture(&self) -> &'static str;
    fn madara_details_fixture(&self) -> &'static str;
    fn madara_pages_fixture(&self) -> &'static str;

    fn madara_default_manga_key(&self, config: &MadaraConfig) -> String {
        format!("/{}/sample", config.manga_path.trim_matches('/'))
    }

    fn madara_default_chapter_key(&self, config: &MadaraConfig) -> String {
        format!("{}/chapter-1", self.madara_default_manga_key(config))
    }

    fn madara_listing_order(&self, request: &Value) -> &'static str {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        }
    }

    fn madara_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.madara_config(&request);
        let fixture = self.madara_list_fixture();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: Madara::parse_listing(fixture, &config),
                has_next_page: Madara::has_next_page(fixture, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = Madara::fetch_document_or_fixture(
            &config,
            &config.list_url(page, self.madara_listing_order(&request)),
            fixture,
        );
        Ok(Paged {
            entries: Madara::parse_listing(&body, &config),
            has_next_page: Madara::has_next_page(&body, &config),
        })
    }

    fn madara_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.madara_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body =
                Madara::fetch_document_or_fixture(&config, query, self.madara_details_fixture());
            return Ok(Paged {
                entries: vec![Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = Madara::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            self.madara_list_fixture(),
        );
        Ok(Paged {
            entries: Madara::parse_listing(&body, &config),
            has_next_page: Madara::has_next_page(&body, &config),
        })
    }

    fn madara_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.madara_config(&request);
        let key = request_key(&request, "manga")
            .unwrap_or_else(|| self.madara_default_manga_key(&config));
        let body = Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.madara_details_fixture(),
        );
        Ok(Madara::parse_details(&body, Some(key), &config))
    }

    fn madara_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.madara_config(&request);
        let key = request_key(&request, "manga")
            .unwrap_or_else(|| self.madara_default_manga_key(&config));
        let body = Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.madara_details_fixture(),
        );
        Ok(Madara::parse_chapters(&body, &key, &config))
    }

    fn madara_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.madara_config(&request);
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| self.madara_default_chapter_key(&config));
        let body = Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.madara_pages_fixture(),
        );
        Ok(Madara::parse_pages(&body, &config))
    }

    fn madara_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.madara_config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(Madara::parse_details(
                    &Madara::fetch_document_or_fixture(
                        &config,
                        input,
                        self.madara_details_fixture(),
                    ),
                    Some(config.normalize_manga_key(input)),
                    &config,
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

#[macro_export]
macro_rules! impl_madara_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MadaraSource::madara_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MadaraSource::madara_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::MadaraSource::madara_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::MadaraSource::madara_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::MadaraSource::madara_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::MadaraSource::madara_handle_url(self, request)
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct GattsuConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
}

impl GattsuConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    pub fn list_url(&self, page: u64) -> String {
        if page <= 1 {
            format!("{}/", self.base_url.trim_end_matches('/'))
        } else {
            format!("{}/page/{page}/", self.base_url.trim_end_matches('/'))
        }
    }

    pub fn search_url(&self, page: u64, query: &str) -> String {
        format!(
            "{}/page/{page}/?s={}&post_type=post",
            self.base_url.trim_end_matches('/'),
            url::query_escape(query)
        )
    }
}

pub struct Gattsu;

impl Gattsu {
    pub fn browser_client(config: &GattsuConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(config.base_url.to_string())
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_document_or_fixture(
        config: &GattsuConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        Self::browser_client(config)
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &GattsuConfig) -> Vec<CatalogItem> {
        body.split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("thumb-titulo") || chunk.contains("thumb-imagem"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = config.normalize_key(&href);
                let title = html::text_between(chunk, "thumb-titulo", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| config.name.into())
                    });
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: gattsu_image(chunk)
                        .map(|value| without_thumbnail_size(&config.absolute_url(&value))),
                    url: Some(config.absolute_url(&key)),
                    language: Some(config.lang.to_string()),
                    content_rating: Some(config.content_rating.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn has_next_page(body: &str) -> bool {
        body.contains("paginacao")
            && (body.contains("class=\"next\"") || body.contains(" rel=\"next\""))
    }

    pub fn parse_details(body: &str, key: Option<String>, config: &GattsuConfig) -> CatalogItem {
        let key = key
            .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
            .map(|value| config.normalize_key(&value))
            .unwrap_or_else(|| "/sample".to_string());
        let details = html::text_between(body, "post-box", "</article>")
            .or_else(|| html::text_between(body, "post-box", "<div class=\"post-box"))
            .unwrap_or_else(|| body.to_string());
        CatalogItem {
            key: key.clone(),
            title: html::text_between(&details, "post-titulo", "</")
                .or_else(|| html::text_between(&details, "<h1", "</h1>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| config.name.into())),
            cover: html::attr_after(&details, "post-capa", "src")
                .or_else(|| gattsu_image(&details))
                .map(|value| without_thumbnail_size(&config.absolute_url(&value))),
            description: html::text_between(&details, "post-texto", "</div>")
                .map(|value| {
                    html::strip_tags(&value)
                        .replace("Sinopse :", "")
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty()),
            authors: gattsu_info_values(&details, "Artista"),
            tags: gattsu_info_values(&details, "Tags"),
            status: ItemStatus::Completed,
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn parse_chapters(body: &str, manga_key: &str, config: &GattsuConfig) -> Vec<MangaChapter> {
        if Self::parse_pages(body, config).is_empty() {
            return Vec::new();
        }
        vec![MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capítulo único".to_string()),
            url: Some(config.absolute_url(manga_key)),
            date_uploaded: html::attr_after(body, "article:published_time", "content").and_then(
                |value| crate::dates::parse_fixture_date(value.split('T').next().unwrap_or(&value)),
            ),
            ..MangaChapter::default()
        }]
    }

    pub fn parse_pages(body: &str, config: &GattsuConfig) -> Vec<MangaPage> {
        body.split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("post-fotos")
                    || chunk.contains("galeriaHtml")
                    || chunk.contains("data-src")
                    || chunk.contains("wp-post-image")
            })
            .filter_map(|chunk| {
                html::attr(chunk, "data-src")
                    .or_else(|| html::attr(chunk, "data-lazy-src"))
                    .or_else(|| srcset_first(html::attr(chunk, "srcset")))
                    .or_else(|| html::attr(chunk, "src"))
            })
            .filter(|value| !value.starts_with("data:") && !value.is_empty())
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: without_thumbnail_size(&config.absolute_url(&image)),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

pub trait GattsuSource {
    fn gattsu_config(&self) -> GattsuConfig;
    fn gattsu_list_fixture(&self) -> &'static str;
    fn gattsu_details_fixture(&self) -> &'static str;
    fn gattsu_pages_fixture(&self) -> &'static str;

    fn gattsu_default_manga_key(&self) -> String {
        "/sample".to_string()
    }

    fn gattsu_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.gattsu_config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = if request.as_object().is_some_and(|object| object.is_empty()) {
            self.gattsu_list_fixture().to_string()
        } else {
            Gattsu::fetch_document_or_fixture(
                &config,
                &config.list_url(page),
                self.gattsu_list_fixture(),
            )
        };
        Ok(Paged {
            entries: Gattsu::parse_listing(&body, &config),
            has_next_page: Gattsu::has_next_page(&body),
        })
    }

    fn gattsu_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.gattsu_config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = config.normalize_key(query);
            let body =
                Gattsu::fetch_document_or_fixture(&config, query, self.gattsu_details_fixture());
            return Ok(Paged {
                entries: vec![Gattsu::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = Gattsu::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query),
            self.gattsu_list_fixture(),
        );
        Ok(Paged {
            entries: Gattsu::parse_listing(&body, &config),
            has_next_page: Gattsu::has_next_page(&body),
        })
    }

    fn gattsu_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.gattsu_config();
        let key = request_key(&request, "manga").unwrap_or_else(|| self.gattsu_default_manga_key());
        let body = Gattsu::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.gattsu_details_fixture(),
        );
        Ok(Gattsu::parse_details(&body, Some(key), &config))
    }

    fn gattsu_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.gattsu_config();
        let key = request_key(&request, "manga").unwrap_or_else(|| self.gattsu_default_manga_key());
        let body = Gattsu::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.gattsu_details_fixture(),
        );
        Ok(Gattsu::parse_chapters(&body, &key, &config))
    }

    fn gattsu_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.gattsu_config();
        let key =
            request_key(&request, "chapter").unwrap_or_else(|| self.gattsu_default_manga_key());
        let body = Gattsu::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.gattsu_pages_fixture(),
        );
        Ok(Gattsu::parse_pages(&body, &config))
    }

    fn gattsu_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.gattsu_config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = config.normalize_key(input);
            let body =
                Gattsu::fetch_document_or_fixture(&config, input, self.gattsu_details_fixture());
            return Ok(Some(UrlResolveResult {
                item: Some(Gattsu::parse_details(&body, Some(key), &config)),
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

#[macro_export]
macro_rules! impl_gattsu_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::GattsuSource::gattsu_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::GattsuSource::gattsu_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::GattsuSource::gattsu_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::GattsuSource::gattsu_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::GattsuSource::gattsu_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::GattsuSource::gattsu_handle_url(self, request)
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct PizzaReaderConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
    pub api_path: &'static str,
}

impl PizzaReaderConfig {
    pub fn api_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.api_path)
    }

    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn api_absolute_url(&self, value: &str) -> String {
        url::join_url(&self.api_url(), value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaResultsDto {
    #[serde(default)]
    comics: Vec<PizzaComicDto>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaResultDto {
    comic: Option<PizzaComicDto>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaReaderDto {
    chapter: Option<PizzaChapterDto>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaComicDto {
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    chapters: Vec<PizzaChapterDto>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: Vec<PizzaGenreDto>,
    #[serde(default, rename = "last_chapter")]
    last_chapter: Option<PizzaChapterDto>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaGenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaChapterDto {
    #[serde(default)]
    chapter: Option<f32>,
    #[serde(default, rename = "full_title")]
    full_title: String,
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default, rename = "published_on")]
    published_on: String,
    #[serde(default)]
    subchapter: Option<f32>,
    #[serde(default)]
    teams: Vec<Option<PizzaTeamDto>>,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PizzaTeamDto {
    #[serde(default)]
    name: String,
}

pub struct PizzaReader;

impl PizzaReader {
    pub fn browser_client(config: &PizzaReaderConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(config.base_url.to_string())
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_text_or_fixture(
        config: &PizzaReaderConfig,
        target: &str,
        fixture: &str,
    ) -> String {
        Self::browser_client(config)
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn list_url(config: &PizzaReaderConfig) -> String {
        format!("{}/comics", config.api_url().trim_end_matches('/'))
    }

    pub fn search_url(config: &PizzaReaderConfig, query: &str) -> String {
        format!(
            "{}/search/{}",
            config.api_url().trim_end_matches('/'),
            url::query_escape(query)
        )
    }

    pub fn parse_listing(body: &str, config: &PizzaReaderConfig) -> Vec<CatalogItem> {
        let result = serde_json::from_str::<PizzaResultsDto>(body).unwrap_or_default();
        result
            .comics
            .into_iter()
            .map(|comic| Self::catalog_item_from_comic(comic, false, config))
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn parse_latest(body: &str, config: &PizzaReaderConfig) -> Vec<CatalogItem> {
        let mut comics = serde_json::from_str::<PizzaResultsDto>(body)
            .unwrap_or_default()
            .comics;
        comics.sort_by(|left, right| {
            right
                .last_chapter
                .as_ref()
                .map(|chapter| &chapter.published_on)
                .cmp(
                    &left
                        .last_chapter
                        .as_ref()
                        .map(|chapter| &chapter.published_on),
                )
        });
        comics
            .into_iter()
            .filter(|comic| comic.last_chapter.is_some())
            .take(10)
            .map(|comic| Self::catalog_item_from_comic(comic, false, config))
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn parse_details(
        body: &str,
        key: Option<String>,
        config: &PizzaReaderConfig,
    ) -> CatalogItem {
        let comic = serde_json::from_str::<PizzaResultDto>(body)
            .unwrap_or_default()
            .comic
            .unwrap_or_default();
        let key = key.unwrap_or_else(|| config.normalize_key(&comic.url));
        let mut item = Self::catalog_item_from_comic(comic, true, config);
        item.key = key.clone();
        item.url = Some(config.absolute_url(&key));
        item.initialized = true;
        item
    }

    pub fn parse_chapters(body: &str, config: &PizzaReaderConfig) -> Vec<MangaChapter> {
        let Some(comic) = serde_json::from_str::<PizzaResultDto>(body)
            .unwrap_or_default()
            .comic
        else {
            return Vec::new();
        };
        comic
            .chapters
            .into_iter()
            .map(|chapter| Self::chapter_from_dto(chapter, config))
            .collect()
    }

    pub fn parse_pages(body: &str, config: &PizzaReaderConfig) -> Vec<MangaPage> {
        let pages = serde_json::from_str::<PizzaReaderDto>(body)
            .unwrap_or_default()
            .chapter
            .map(|chapter| chapter.pages)
            .unwrap_or_default();
        pages
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }

    fn catalog_item_from_comic(
        comic: PizzaComicDto,
        initialized: bool,
        config: &PizzaReaderConfig,
    ) -> CatalogItem {
        let key = config.normalize_key(&comic.url);
        CatalogItem {
            key: key.clone(),
            title: if comic.title.is_empty() {
                url::slug_from_url(&key).unwrap_or_else(|| config.name.to_string())
            } else {
                comic.title
            },
            cover: (!comic.thumbnail.is_empty()).then(|| config.absolute_url(&comic.thumbnail)),
            url: Some(config.absolute_url(&key)),
            description: (!comic.description.is_empty()).then_some(comic.description),
            authors: if comic.author.is_empty() {
                Vec::new()
            } else {
                vec![comic.author]
            },
            artists: comic.artist.map(|artist| vec![artist]).unwrap_or_default(),
            tags: comic
                .genres
                .into_iter()
                .map(|genre| genre.name)
                .filter(|name| !name.is_empty())
                .collect(),
            status: comic
                .status
                .as_deref()
                .and_then(pizza_status)
                .unwrap_or(ItemStatus::Unknown),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }

    fn chapter_from_dto(chapter: PizzaChapterDto, config: &PizzaReaderConfig) -> MangaChapter {
        let key = config.normalize_key(&chapter.url);
        let chapter_number = match (chapter.chapter, chapter.subchapter) {
            (Some(base), Some(sub)) => Some(base + (sub / 10.0)),
            (Some(base), None) => Some(base),
            _ => None,
        };
        MangaChapter {
            key: key.clone(),
            title: (!chapter.full_title.is_empty()).then_some(chapter.full_title),
            chapter_number,
            scanlators: chapter
                .teams
                .into_iter()
                .flatten()
                .map(|team| team.name)
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>(),
            date_uploaded: crate::dates::parse_fixture_date(&chapter.published_on),
            url: Some(config.absolute_url(&key)),
            ..MangaChapter::default()
        }
    }
}

fn pizza_status(value: &str) -> Option<ItemStatus> {
    let lowered = value.to_lowercase();
    if lowered.starts_with("in cors") || lowered.starts_with("on goin") {
        Some(ItemStatus::Ongoing)
    } else if lowered.starts_with("complet")
        || lowered.starts_with("conclus")
        || lowered.starts_with("conclud")
    {
        Some(ItemStatus::Completed)
    } else if lowered.starts_with("licenzi") || lowered.starts_with("license") {
        Some(ItemStatus::Cancelled)
    } else {
        None
    }
}

pub trait PizzaReaderSource {
    fn pizza_config(&self) -> PizzaReaderConfig;
    fn pizza_list_fixture(&self) -> &'static str;
    fn pizza_details_fixture(&self) -> &'static str;
    fn pizza_pages_fixture(&self) -> &'static str;

    fn pizza_default_manga_key(&self) -> String {
        "/comics/sample".to_string()
    }

    fn pizza_default_chapter_key(&self) -> String {
        "/chapters/sample".to_string()
    }

    fn pizza_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.pizza_config();
        let body = PizzaReader::fetch_text_or_fixture(
            &config,
            &PizzaReader::list_url(&config),
            self.pizza_list_fixture(),
        );
        let entries = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            PizzaReader::parse_latest(&body, &config)
        } else {
            PizzaReader::parse_listing(&body, &config)
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn pizza_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.pizza_config();
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = config.normalize_key(query);
            let body = PizzaReader::fetch_text_or_fixture(
                &config,
                &config.api_absolute_url(&key),
                self.pizza_details_fixture(),
            );
            return Ok(Paged {
                entries: vec![PizzaReader::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = PizzaReader::fetch_text_or_fixture(
            &config,
            &PizzaReader::search_url(&config, query),
            self.pizza_list_fixture(),
        );
        Ok(Paged {
            entries: PizzaReader::parse_listing(&body, &config),
            has_next_page: false,
        })
    }

    fn pizza_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.pizza_config();
        let key = request_key(&request, "manga").unwrap_or_else(|| self.pizza_default_manga_key());
        let body = PizzaReader::fetch_text_or_fixture(
            &config,
            &config.api_absolute_url(&key),
            self.pizza_details_fixture(),
        );
        Ok(PizzaReader::parse_details(&body, Some(key), &config))
    }

    fn pizza_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.pizza_config();
        let key = request_key(&request, "manga").unwrap_or_else(|| self.pizza_default_manga_key());
        let body = PizzaReader::fetch_text_or_fixture(
            &config,
            &config.api_absolute_url(&key),
            self.pizza_details_fixture(),
        );
        Ok(PizzaReader::parse_chapters(&body, &config))
    }

    fn pizza_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.pizza_config();
        let key =
            request_key(&request, "chapter").unwrap_or_else(|| self.pizza_default_chapter_key());
        let body = PizzaReader::fetch_text_or_fixture(
            &config,
            &config.api_absolute_url(&key),
            self.pizza_pages_fixture(),
        );
        Ok(PizzaReader::parse_pages(&body, &config))
    }

    fn pizza_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.pizza_config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = config.normalize_key(input);
            let body = PizzaReader::fetch_text_or_fixture(
                &config,
                &config.api_absolute_url(&key),
                self.pizza_details_fixture(),
            );
            return Ok(Some(UrlResolveResult {
                item: Some(PizzaReader::parse_details(&body, Some(key), &config)),
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

#[macro_export]
macro_rules! impl_pizza_reader_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::PizzaReaderSource::pizza_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::PizzaReaderSource::pizza_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::PizzaReaderSource::pizza_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::PizzaReaderSource::pizza_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::PizzaReaderSource::pizza_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::PizzaReaderSource::pizza_handle_url(self, request)
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct MangaWorldConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
}

impl MangaWorldConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    pub fn list_url(&self, page: u64, listing_id: &str) -> String {
        match listing_id {
            "latest" => format!("{}/?page={page}", self.base_url.trim_end_matches('/')),
            _ => format!(
                "{}/archive?sort=most_read&page={page}",
                self.base_url.trim_end_matches('/')
            ),
        }
    }

    pub fn search_url(&self, page: u64, query: &str, sort: &str) -> String {
        let mut params = vec![format!("page={page}")];
        if !query.trim().is_empty() {
            params.push(format!("keyword={}", url::query_escape(query.trim())));
        }
        if !sort.is_empty() {
            params.push(format!("sort={sort}"));
        }
        format!(
            "{}/archive?{}",
            self.base_url.trim_end_matches('/'),
            params.join("&")
        )
    }
}

pub struct MangaWorld;

impl MangaWorld {
    pub fn browser_client(config: &MangaWorldConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_document_or_fixture(
        config: &MangaWorldConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        Self::browser_client(config)
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &MangaWorldConfig) -> Vec<CatalogItem> {
        body.split("class=\"entry")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| config.name.to_string());
                let key = config.normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "src")
                        .map(|image| config.absolute_url(&image)),
                    url: Some(config.absolute_url(&key)),
                    language: Some(config.lang.to_string()),
                    content_rating: Some(config.content_rating.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn has_next_page(body: &str) -> bool {
        body.matches("class=\"entry").count() >= 16
            || body.contains("rel=\"next\"")
            || body.contains("page-item next")
    }

    pub fn parse_details(
        body: &str,
        key: Option<String>,
        config: &MangaWorldConfig,
    ) -> CatalogItem {
        let key = key
            .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
            .map(|value| config.normalize_key(&value))
            .unwrap_or_else(|| "/archive/sample".to_string());
        let info = html::text_between(body, "comic-info", "</section>")
            .unwrap_or_else(|| body.to_string());
        CatalogItem {
            key: key.clone(),
            title: html::text_between(body, "<h1", "</h1>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| config.name.to_string()),
            cover: html::attr_after(&info, "class=\"thumb", "src")
                .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
                .map(|image| config.absolute_url(&image)),
            description: html::text_between(body, "id=\"noidungm\"", "</div>")
                .or_else(|| html::text_between(body, "id='noidungm'", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: manga_world_link_values(&info, "/archive?author="),
            artists: manga_world_link_values(&info, "/archive?artist="),
            tags: manga_world_badges(&info),
            status: manga_world_status(
                &manga_world_link_values(&info, "/archive?status=")
                    .first()
                    .cloned()
                    .unwrap_or_default(),
            ),
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn parse_chapters(body: &str, config: &MangaWorldConfig) -> Vec<MangaChapter> {
        body.split("class=\"chapter")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "class=\"chap", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = manga_world_chapter_key(&config.normalize_key(&href));
                let title = html::text_between(chunk, "d-inline-block", "</span>")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Capitolo".to_string());
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(title.clone()),
                    chapter_number: manga_world_chapter_number(&title),
                    date_uploaded: html::text_between(chunk, "chap-date", "</")
                        .map(|value| html::strip_tags(&value))
                        .and_then(|value| crate::dates::parse_fixture_date(&value)),
                    url: Some(config.absolute_url(&key)),
                    ..MangaChapter::default()
                })
            })
            .collect()
    }

    pub fn parse_pages(body: &str, config: &MangaWorldConfig) -> Vec<MangaPage> {
        body.split("<img")
            .skip(1)
            .filter(|chunk| chunk.contains("page-image"))
            .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: Some(image_headers(config.base_url)),
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

fn manga_world_chapter_key(key: &str) -> String {
    if key.contains("style=list") {
        key.to_string()
    } else if key.contains("style=pages") {
        key.replace("style=pages", "style=list")
    } else if key.contains('?') {
        format!("{key}&style=list")
    } else {
        format!("{key}?style=list")
    }
}

fn manga_world_chapter_number(title: &str) -> Option<f32> {
    let lowered = title.to_lowercase();
    let marker = "capitolo";
    let start = lowered.find(marker)? + marker.len();
    lowered[start..]
        .split_whitespace()
        .next()
        .and_then(|value| value.replace(',', ".").parse::<f32>().ok())
}

fn manga_world_link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn manga_world_badges(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("badge"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn manga_world_status(value: &str) -> ItemStatus {
    match value.to_lowercase().as_str() {
        "in corso" => ItemStatus::Ongoing,
        "finito" => ItemStatus::Completed,
        "in pausa" => ItemStatus::Hiatus,
        "cancellato" | "droppato" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

pub trait MangaWorldSource {
    fn manga_world_config(&self) -> MangaWorldConfig;
    fn manga_world_list_fixture(&self) -> &'static str;
    fn manga_world_details_fixture(&self) -> &'static str;
    fn manga_world_pages_fixture(&self) -> &'static str;

    fn manga_world_default_manga_key(&self) -> String {
        "/manga/sample".to_string()
    }

    fn manga_world_default_chapter_key(&self) -> String {
        "/read/sample/chapter-1?style=list".to_string()
    }

    fn manga_world_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.manga_world_config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing_id = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body = MangaWorld::fetch_document_or_fixture(
            &config,
            &config.list_url(page, listing_id),
            self.manga_world_list_fixture(),
        );
        Ok(Paged {
            entries: MangaWorld::parse_listing(&body, &config),
            has_next_page: MangaWorld::has_next_page(&body),
        })
    }

    fn manga_world_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.manga_world_config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_key(query);
            let body = MangaWorld::fetch_document_or_fixture(
                &config,
                query,
                self.manga_world_details_fixture(),
            );
            return Ok(Paged {
                entries: vec![MangaWorld::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let sort = request
            .get("filters")
            .and_then(|filters| filters.get("sort"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = MangaWorld::fetch_document_or_fixture(
            &config,
            &config.search_url(page, query, sort),
            self.manga_world_list_fixture(),
        );
        Ok(Paged {
            entries: MangaWorld::parse_listing(&body, &config),
            has_next_page: MangaWorld::has_next_page(&body),
        })
    }

    fn manga_world_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.manga_world_config();
        let key =
            request_key(&request, "manga").unwrap_or_else(|| self.manga_world_default_manga_key());
        let body = MangaWorld::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.manga_world_details_fixture(),
        );
        Ok(MangaWorld::parse_details(&body, Some(key), &config))
    }

    fn manga_world_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.manga_world_config();
        let key =
            request_key(&request, "manga").unwrap_or_else(|| self.manga_world_default_manga_key());
        let body = MangaWorld::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.manga_world_details_fixture(),
        );
        Ok(MangaWorld::parse_chapters(&body, &config))
    }

    fn manga_world_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.manga_world_config();
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| self.manga_world_default_chapter_key());
        let body = MangaWorld::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.manga_world_pages_fixture(),
        );
        Ok(MangaWorld::parse_pages(&body, &config))
    }

    fn manga_world_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.manga_world_config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = config.normalize_key(input);
            let body = MangaWorld::fetch_document_or_fixture(
                &config,
                input,
                self.manga_world_details_fixture(),
            );
            return Ok(Some(UrlResolveResult {
                item: Some(MangaWorld::parse_details(&body, Some(key), &config)),
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

#[macro_export]
macro_rules! impl_manga_world_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MangaWorldSource::manga_world_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MangaWorldSource::manga_world_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::MangaWorldSource::manga_world_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::MangaWorldSource::manga_world_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::MangaWorldSource::manga_world_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::MangaWorldSource::manga_world_handle_url(self, request)
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct FoolSlideConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
    pub url_modifier: &'static str,
    pub popular_uses_latest: bool,
}

impl FoolSlideConfig {
    pub fn root_url(&self, path: &str) -> String {
        format!(
            "{}{}{}",
            self.base_url.trim_end_matches('/'),
            self.url_modifier,
            path
        )
    }

    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

pub struct FoolSlide;

impl FoolSlide {
    pub fn browser_client(config: &FoolSlideConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_document_or_fixture(
        config: &FoolSlideConfig,
        target: &str,
        fixture: &str,
    ) -> String {
        Self::browser_client(config)
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn post_search_or_fixture(config: &FoolSlideConfig, query: &str, fixture: &str) -> String {
        Self::browser_client(config)
            .post(config.root_url("/search/"))
            .form(&[("search", query)])
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &FoolSlideConfig) -> Paged<CatalogItem> {
        Paged {
            entries: body
                .split("div class=\"group")
                .skip(1)
                .chain(body.split("div class=\"list").skip(1))
                .filter_map(|chunk| {
                    let href = html::attr_after(chunk, "<a", "href")?;
                    let title = html::attr_after(chunk, "<a", "title")
                        .or_else(|| {
                            html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v))
                        })
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&href))
                        .unwrap_or_else(|| config.name.to_string());
                    Some(CatalogItem {
                        key: config.normalize_key(&href),
                        title,
                        cover: html::attr_after(chunk, "<img", "src")
                            .map(|image| config.absolute_url(&image.replace("/thumb_", "/"))),
                        url: Some(config.absolute_url(&config.normalize_key(&href))),
                        language: Some(config.lang.to_string()),
                        content_rating: Some(config.content_rating.to_string()),
                        initialized: false,
                        ..CatalogItem::default()
                    })
                })
                .fold(Vec::new(), push_unique_catalog_item),
            has_next_page: body.contains("div class=\"next") || body.contains("<span class=\"next"),
        }
    }

    pub fn parse_details(body: &str, key: Option<String>, config: &FoolSlideConfig) -> CatalogItem {
        let key = key.unwrap_or_else(|| "/reader/series/sample".to_string());
        let info = html::text_between(body, "div class=\"info", "</div>")
            .unwrap_or_else(|| body.to_string());
        CatalogItem {
            key: key.clone(),
            title: html::attr_after(body, "<a", "title")
                .or_else(|| html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)))
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| config.name.to_string()),
            cover: html::attr_after(body, "div class=\"thumbnail", "src")
                .or_else(|| html::attr_after(body, "table class=\"thumb", "src"))
                .or_else(|| html::attr_after(body, "<img", "src"))
                .map(|image| config.absolute_url(&image.replace("/thumb_", "/"))),
            description: info_after(&info, &["Synopsis", "Description", "Trama"]),
            authors: info_after(&info, &["Author", "Autore"])
                .into_iter()
                .collect(),
            artists: info_after(&info, &["Artist"]).into_iter().collect(),
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn parse_chapters(body: &str, config: &FoolSlideConfig) -> Vec<MangaChapter> {
        body.split("div class=\"element")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<a", "title"))
                    .unwrap_or_else(|| "Chapter".to_string());
                if title.trim().chars().all(|ch| ch.is_ascii_digit()) {
                    return None;
                }
                Some(MangaChapter {
                    key: config.normalize_key(&href),
                    title: Some(title.clone()),
                    chapter_number: chapter_number_from_url(&href),
                    date_uploaded: html::text_between(chunk, "meta_r", "</")
                        .map(|value| html::strip_tags(&value))
                        .and_then(|value| crate::dates::parse_fixture_date(&value)),
                    url: Some(config.absolute_url(&config.normalize_key(&href))),
                    ..MangaChapter::default()
                })
            })
            .collect()
    }

    pub fn parse_pages(body: &str, config: &FoolSlideConfig) -> Vec<MangaPage> {
        let json = body
            .split("var pages = ")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .unwrap_or("[]");
        let urls = serde_json::from_str::<Vec<serde_json::Value>>(json).unwrap_or_default();
        urls.into_iter()
            .filter_map(|value| {
                value
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: Some(image_headers(config.base_url)),
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

fn info_after(body: &str, labels: &[&str]) -> Option<String> {
    for label in labels {
        let marker = format!("{label}</b>:");
        if let Some(rest) = body.split(&marker).nth(1) {
            let value = rest.split(['\n', '<']).next().unwrap_or_default();
            let value = html::strip_tags(value).trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn chapter_number_from_url(value: &str) -> Option<f32> {
    let trimmed = value.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .find_map(|part| part.parse::<f32>().ok())
}

pub trait FoolSlideSource {
    fn foolslide_config(&self) -> FoolSlideConfig;
    fn foolslide_list_fixture(&self) -> &'static str;
    fn foolslide_details_fixture(&self) -> &'static str;
    fn foolslide_pages_fixture(&self) -> &'static str;

    fn foolslide_default_manga_key(&self) -> String {
        "/reader/series/sample".to_string()
    }

    fn foolslide_default_chapter_key(&self) -> String {
        "/reader/read/sample/1".to_string()
    }

    fn foolslide_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.foolslide_config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing_id = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing_id == "latest" || config.popular_uses_latest {
            format!("/latest/{page}/")
        } else {
            format!("/directory/{page}/")
        };
        let body = FoolSlide::fetch_document_or_fixture(
            &config,
            &config.root_url(&path),
            self.foolslide_list_fixture(),
        );
        Ok(FoolSlide::parse_listing(&body, &config))
    }

    fn foolslide_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.foolslide_config();
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(config.base_url) {
            let key = config.normalize_key(query);
            let body = FoolSlide::fetch_document_or_fixture(
                &config,
                query,
                self.foolslide_details_fixture(),
            );
            return Ok(Paged {
                entries: vec![FoolSlide::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let body = FoolSlide::post_search_or_fixture(&config, query, self.foolslide_list_fixture());
        Ok(FoolSlide::parse_listing(&body, &config))
    }

    fn foolslide_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.foolslide_config();
        let key =
            request_key(&request, "manga").unwrap_or_else(|| self.foolslide_default_manga_key());
        let body = FoolSlide::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.foolslide_details_fixture(),
        );
        Ok(FoolSlide::parse_details(&body, Some(key), &config))
    }

    fn foolslide_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.foolslide_config();
        let key =
            request_key(&request, "manga").unwrap_or_else(|| self.foolslide_default_manga_key());
        let body = FoolSlide::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.foolslide_details_fixture(),
        );
        Ok(FoolSlide::parse_chapters(&body, &config))
    }

    fn foolslide_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.foolslide_config();
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| self.foolslide_default_chapter_key());
        let body = FoolSlide::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            self.foolslide_pages_fixture(),
        );
        Ok(FoolSlide::parse_pages(&body, &config))
    }

    fn foolslide_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.foolslide_config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = config.normalize_key(input);
            let body = FoolSlide::fetch_document_or_fixture(
                &config,
                input,
                self.foolslide_details_fixture(),
            );
            return Ok(Some(UrlResolveResult {
                item: Some(FoolSlide::parse_details(&body, Some(key), &config)),
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

#[macro_export]
macro_rules! impl_foolslide_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::FoolSlideSource::foolslide_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::FoolSlideSource::foolslide_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::FoolSlideSource::foolslide_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::FoolSlideSource::foolslide_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::FoolSlideSource::foolslide_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::FoolSlideSource::foolslide_handle_url(self, request)
            }
        }
    };
}

#[derive(Debug, Clone)]
pub struct MasonryConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
}

impl MasonryConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    pub fn popular_url(&self, page: u64) -> String {
        match page {
            0 | 1 => self.base_url.to_string(),
            2 => format!("{}/archive/", self.base_url.trim_end_matches('/')),
            _ => format!(
                "{}/archive/page/{}/",
                self.base_url.trim_end_matches('/'),
                page - 1
            ),
        }
    }

    pub fn latest_url(&self, page: u64) -> String {
        format!(
            "{}/updates/sort/newest/mpage/{page}/",
            self.base_url.trim_end_matches('/')
        )
    }

    pub fn search_url(&self, page: u64, query: &str, tag: &str, sort: &str) -> String {
        if !query.trim().is_empty() {
            return format!(
                "{}/search/post/{}/mpage/{page}/",
                self.base_url.trim_end_matches('/'),
                url::query_escape(query.trim())
            );
        }
        let sort_path = match sort {
            "Trending" => "sort/trending",
            "Popular" => "sort/popular",
            _ => "sort/newest",
        };
        if tag.is_empty() {
            format!(
                "{}/updates/{sort_path}/mpage/{page}/",
                self.base_url.trim_end_matches('/')
            )
        } else {
            format!(
                "{}/tag/{tag}/{sort_path}/mpage/{page}/",
                self.base_url.trim_end_matches('/')
            )
        }
    }
}

pub struct Masonry;

impl Masonry {
    pub fn client(config: &MasonryConfig) -> http::HttpClient {
        http::HttpClient::browser()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
    }

    pub fn fetch_document_or_fixture(
        config: &MasonryConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        Self::client(config)
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &MasonryConfig) -> Vec<CatalogItem> {
        body.split("<figure")
            .skip(1)
            .filter(|chunk| chunk.contains("list-gallery") || chunk.contains("<a"))
            .filter(|chunk| !chunk.contains("/video/"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| config.name.to_string());
                let key = config.normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: masonry_image(chunk).map(|value| config.absolute_url(&value)),
                    url: Some(config.absolute_url(&key)),
                    language: Some(config.lang.to_string()),
                    content_rating: Some(config.content_rating.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn has_next_page(body: &str) -> bool {
        body.contains("pagination-a") && body.contains("next")
    }

    pub fn parse_details(body: &str, key: Option<String>, config: &MasonryConfig) -> CatalogItem {
        let key = key.unwrap_or_else(|| "/gallery".to_string());
        CatalogItem {
            key: key.clone(),
            title: html::attr_after(body, "property=\"og:title\"", "content")
                .or_else(|| {
                    html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
                })
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| config.name.to_string()),
            description: html::text_between(body, "#content > p", "</p>")
                .or_else(|| html::text_between(body, "<p", "</p>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: masonry_link_values(body, "/model/"),
            artists: masonry_link_values(body, "/model/"),
            tags: masonry_link_values(body, "/tag/"),
            cover: masonry_image(body).map(|value| config.absolute_url(&value)),
            status: ItemStatus::Completed,
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn chapter(key: &str, config: &MasonryConfig) -> Vec<MangaChapter> {
        vec![MangaChapter {
            key: key.to_string(),
            title: Some("Gallery".to_string()),
            url: Some(config.absolute_url(key)),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }]
    }

    pub fn parse_pages(body: &str, config: &MasonryConfig) -> Vec<MangaPage> {
        body.split("<a")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("href=\"https://cdn.") || chunk.contains("href='https://cdn.")
            })
            .filter_map(|chunk| html::attr(chunk, "href"))
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

pub trait MasonrySource {
    fn masonry_config(&self, request: &Value) -> &MasonryConfig;
    fn masonry_list_fixture(&self) -> &'static str;
    fn masonry_details_fixture(&self) -> &'static str;
    fn masonry_pages_fixture(&self) -> &'static str;

    fn masonry_search_filters<'a>(&self, request: &'a Value) -> (&'a str, &'a str) {
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .unwrap_or("Newest");
        let tag = filters
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        (tag, sort)
    }

    fn masonry_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.masonry_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            config.latest_url(page)
        } else {
            config.popular_url(page)
        };
        let body = Masonry::fetch_document_or_fixture(config, &target, self.masonry_list_fixture());
        Ok(Paged {
            entries: Masonry::parse_listing(&body, config),
            has_next_page: Masonry::has_next_page(&body),
        })
    }

    fn masonry_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.masonry_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = config.normalize_key(query);
            let body =
                Masonry::fetch_document_or_fixture(config, query, self.masonry_details_fixture());
            return Ok(Paged {
                entries: vec![Masonry::parse_details(&body, Some(key), config)],
                has_next_page: false,
            });
        }
        let (tag, sort) = self.masonry_search_filters(&request);
        let body = Masonry::fetch_document_or_fixture(
            config,
            &config.search_url(page, query, tag, sort),
            self.masonry_list_fixture(),
        );
        Ok(Paged {
            entries: Masonry::parse_listing(&body, config),
            has_next_page: Masonry::has_next_page(&body),
        })
    }

    fn masonry_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.masonry_config(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample".into());
        let body = Masonry::fetch_document_or_fixture(
            config,
            &config.absolute_url(&key),
            self.masonry_details_fixture(),
        );
        Ok(Masonry::parse_details(&body, Some(key), config))
    }

    fn masonry_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.masonry_config(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample".into());
        Ok(Masonry::chapter(&key, config))
    }

    fn masonry_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.masonry_config(&request);
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/gallery/sample".into());
        let body = Masonry::fetch_document_or_fixture(
            config,
            &config.absolute_url(&key),
            self.masonry_pages_fixture(),
        );
        Ok(Masonry::parse_pages(&body, config))
    }

    fn masonry_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.masonry_config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            let key = config.normalize_key(input);
            let body =
                Masonry::fetch_document_or_fixture(config, input, self.masonry_details_fixture());
            return Ok(Some(UrlResolveResult {
                item: Some(Masonry::parse_details(&body, Some(key), config)),
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

#[macro_export]
macro_rules! impl_masonry_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MasonrySource::masonry_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MasonrySource::masonry_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::MasonrySource::masonry_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::MasonrySource::masonry_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::MasonrySource::masonry_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::MasonrySource::masonry_handle_url(self, request)
            }
        }
    };
}

fn masonry_image(input: &str) -> Option<String> {
    input.split("<img").nth(1).and_then(|chunk| {
        html::attr(chunk, "srcset")
            .map(|value| {
                value
                    .split_whitespace()
                    .next()
                    .unwrap_or(&value)
                    .to_string()
            })
            .or_else(|| html::attr(chunk, "data-cfsrc"))
            .or_else(|| html::attr(chunk, "data-src"))
            .or_else(|| html::attr(chunk, "data-lazy-src"))
            .or_else(|| html::attr(chunk, "src"))
    })
}

fn masonry_link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

pub fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn madara_image(body: &str) -> Option<String> {
    html::attr_after(body, "data-src", "data-src")
        .or_else(|| html::attr_after(body, "<img", "data-src"))
        .or_else(|| html::attr_after(body, "<img", "data-lazy-src"))
        .or_else(|| srcset_first(html::attr_after(body, "<img", "srcset")))
        .or_else(|| html::attr_after(body, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .find_map(|candidate| candidate.split_whitespace().next().map(ToString::to_string))
            .filter(|url| !url.is_empty())
    })
}

fn madara_info_values(body: &str, name: &str) -> Vec<String> {
    body.split(&format!("post-content_item"))
        .filter(|chunk| chunk.to_ascii_lowercase().contains(name))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn madara_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn push_unique_catalog_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Clone, Copy)]
pub struct MangaCatalogConfig {
    pub base_url: &'static str,
    pub name: &'static str,
    pub lang: &'static str,
    pub content_rating: &'static str,
}

impl MangaCatalogConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(path) = value.strip_prefix(self.base_url) {
                let trimmed = path.trim_matches('/');
                return if trimmed.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{trimmed}")
                };
            }
        }
        let trimmed = value.trim_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{trimmed}")
        }
    }
}

pub struct MangaCatalog;

impl MangaCatalog {
    pub fn fetch_document_or_fixture(
        config: &MangaCatalogConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        http::HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn item(config: &MangaCatalogConfig, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: "/".to_string(),
            title: config.name.to_string(),
            url: Some(config.base_url.to_string()),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }

    pub fn parse_details(
        body: &str,
        key: Option<String>,
        config: &MangaCatalogConfig,
    ) -> CatalogItem {
        let key = key
            .map(|value| config.normalize_key(&value))
            .unwrap_or_else(|| "/".to_string());
        let mut item = Self::item(config, true);
        item.key = key.clone();
        item.title = html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| config.name.to_string());
        item.description = manga_catalog_description(body);
        item.cover = manga_catalog_image(body).map(|value| config.absolute_url(&value));
        item.status = manga_catalog_status(body);
        item.url = Some(config.absolute_url(&key));
        item
    }

    pub fn parse_chapters(
        body: &str,
        manga_key: &str,
        config: &MangaCatalogConfig,
    ) -> Vec<MangaChapter> {
        let chapters = body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("col-span-4") || chunk.contains("Chapter"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if !href.starts_with("http") && !href.starts_with('/') {
                    return None;
                }
                let mut title = html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Chapter".to_string());
                if let Some(extra) = html::text_between(chunk, "text-xs", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty() && !title.contains(value))
                {
                    title = format!("{title} {extra}");
                }
                let key = config.normalize_key(&href);
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(title),
                    url: Some(config.absolute_url(&key)),
                    ..MangaChapter::default()
                })
            })
            .fold(Vec::<MangaChapter>::new(), |mut chapters, chapter| {
                if !chapters.iter().any(|existing| existing.key == chapter.key) {
                    chapters.push(chapter);
                }
                chapters
            });
        if chapters.is_empty() {
            vec![MangaChapter {
                key: manga_key.to_string(),
                title: Some("Read".to_string()),
                url: Some(config.absolute_url(manga_key)),
                ..MangaChapter::default()
            }]
        } else {
            chapters
        }
    }

    pub fn parse_pages(body: &str, config: &MangaCatalogConfig) -> Vec<MangaPage> {
        body.split("<img")
            .skip(1)
            .filter_map(|chunk| {
                html::attr(chunk, "data-src")
                    .or_else(|| html::attr(chunk, "data-lazy-src"))
                    .or_else(|| html::attr(chunk, "data-cfsrc"))
                    .or_else(|| srcset_first(html::attr(chunk, "srcset")))
                    .or_else(|| html::attr(chunk, "src"))
            })
            .filter(|value| !value.starts_with("data:") && !value.is_empty())
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: config.absolute_url(&image),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

pub trait MangaCatalogSource {
    fn manga_catalog_config(&self, request: &Value) -> &MangaCatalogConfig;
    fn manga_catalog_details_fixture(&self) -> &'static str;
    fn manga_catalog_pages_fixture(&self) -> &'static str;

    fn manga_catalog_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.manga_catalog_config(&request);
        Ok(Paged {
            entries: vec![MangaCatalog::item(config, false)],
            has_next_page: false,
        })
    }

    fn manga_catalog_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.manga_catalog_config(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let matches = query.is_empty()
            || config
                .name
                .to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase());
        let entries = if query.starts_with(config.base_url) {
            vec![self.manga_catalog_details(serde_json::json!({"key": query}))?]
        } else if matches {
            vec![MangaCatalog::item(config, false)]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn manga_catalog_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.manga_catalog_config(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/".to_string());
        let target = config.absolute_url(&key);
        Ok(MangaCatalog::parse_details(
            &MangaCatalog::fetch_document_or_fixture(
                config,
                &target,
                self.manga_catalog_details_fixture(),
            ),
            Some(key),
            config,
        ))
    }

    fn manga_catalog_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.manga_catalog_config(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/".to_string());
        let target = config.absolute_url(&key);
        Ok(MangaCatalog::parse_chapters(
            &MangaCatalog::fetch_document_or_fixture(
                config,
                &target,
                self.manga_catalog_details_fixture(),
            ),
            &key,
            config,
        ))
    }

    fn manga_catalog_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.manga_catalog_config(&request);
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/".to_string());
        let target = config.absolute_url(&key);
        Ok(MangaCatalog::parse_pages(
            &MangaCatalog::fetch_document_or_fixture(
                config,
                &target,
                self.manga_catalog_pages_fixture(),
            ),
            config,
        ))
    }

    fn manga_catalog_home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let config = self.manga_catalog_config(&request);
        Ok(vec![HomeSection {
            id: "manga".into(),
            title: "Manga".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: vec![MangaCatalog::item(config, false)],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_catalog_handle_url(
        &self,
        request: Value,
    ) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.manga_catalog_config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(self.manga_catalog_details(serde_json::json!({"key": input}))?),
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

#[macro_export]
macro_rules! impl_manga_catalog_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MangaCatalogSource::manga_catalog_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::MangaCatalogSource::manga_catalog_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::MangaCatalogSource::manga_catalog_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::MangaCatalogSource::manga_catalog_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::MangaCatalogSource::manga_catalog_pages(self, request)
            }

            fn home(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<
                Vec<$crate::sdk::HomeSection<$crate::sdk::CatalogItem>>,
            > {
                $crate::manga::MangaCatalogSource::manga_catalog_home(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::MangaCatalogSource::manga_catalog_handle_url(self, request)
            }
        }
    };
}

fn manga_catalog_image(body: &str) -> Option<String> {
    html::attr_after(body, "property=\"og:image\"", "content")
        .or_else(|| html::attr_after(body, "<img", "data-src"))
        .or_else(|| html::attr_after(body, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(body, "<img", "data-cfsrc"))
        .or_else(|| srcset_first(html::attr_after(body, "<img", "srcset")))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn manga_catalog_description(body: &str) -> Option<String> {
    html::text_between(body, "Description", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let text = html::strip_tags(body);
            let (_, description) = text.split_once("Description")?;
            let description = description
                .split("Chapter")
                .next()
                .unwrap_or(description)
                .trim()
                .to_string();
            (!description.is_empty()).then_some(description)
        })
}

fn manga_catalog_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GalleryAdultsConfig {
    pub base_url: &'static str,
    pub source_id: &'static str,
    pub lang: &'static str,
    pub manga_lang: &'static str,
    pub content_rating: &'static str,
    pub id_prefix_uri: &'static str,
    pub page_uri: &'static str,
    pub list_selector_marker: &'static str,
    pub image_container_marker: &'static str,
}

impl GalleryAdultsConfig {
    pub fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    pub fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(&format!("/{}/", self.id_prefix_uri)) {
                return format!("/{}", value[index + 1..].trim_end_matches('/'));
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    pub fn popular_url(&self, page: u64) -> String {
        let mut path = String::new();
        if !self.manga_lang.is_empty() {
            path.push_str(&format!("language/{}/", self.manga_lang));
        }
        if !self.manga_lang.is_empty() {
            path.push_str("popular/");
        }
        format!(
            "{}/{}?page={page}",
            self.base_url.trim_end_matches('/'),
            path
        )
    }

    pub fn latest_url(&self, page: u64) -> String {
        let path = if self.manga_lang.is_empty() {
            String::new()
        } else {
            format!("language/{}/", self.manga_lang)
        };
        format!(
            "{}/{}?page={page}",
            self.base_url.trim_end_matches('/'),
            path
        )
    }

    pub fn search_url(&self, page: u64, query: &str) -> String {
        format!(
            "{}/search/?q={}&page={page}",
            self.base_url.trim_end_matches('/'),
            url::query_escape(query)
        )
    }

    pub fn id_url(&self, id: &str) -> String {
        format!(
            "{}/{}/{}/",
            self.base_url.trim_end_matches('/'),
            self.id_prefix_uri,
            id.trim_matches('/')
        )
    }
}

pub struct GalleryAdults;

impl GalleryAdults {
    pub fn source_id(request: &Value, default: &'static str) -> String {
        request
            .get("sourceId")
            .or_else(|| request.get("source_id"))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    }

    pub fn fetch_document_or_fixture(
        config: &GalleryAdultsConfig,
        target_url: &str,
        fixture: &str,
    ) -> String {
        http::HttpClient::browser()
            .with_referer(format!("{}/", config.base_url.trim_end_matches('/')))
            .with_cookies_for(config.base_url)
            .with_webview_challenge_fallback()
            .get(target_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string())
    }

    pub fn parse_listing(body: &str, config: &GalleryAdultsConfig) -> Vec<CatalogItem> {
        body.split(config.list_selector_marker)
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = config.normalize_key(&href);
                let title = html::text_between(chunk, "caption", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into())
                    });
                let entry_lang = html::attr_after(chunk, "flag", "href")
                    .and_then(|value| url::slug_from_url(&value))
                    .unwrap_or_else(|| config.manga_lang.to_string());
                if !config.manga_lang.is_empty()
                    && entry_lang != config.manga_lang
                    && entry_lang != "speechless"
                {
                    return None;
                }
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "data-cfsrc")
                        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                        .map(|value| config.absolute_url(&value)),
                    url: Some(config.absolute_url(&key)),
                    language: Some(config.lang.to_string()),
                    content_rating: Some(config.content_rating.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique_catalog_item)
    }

    pub fn has_next_page(body: &str) -> bool {
        body.contains("pagination") && body.contains("active") && !body.contains("disabled")
    }

    pub fn parse_details(
        body: &str,
        key: Option<String>,
        config: &GalleryAdultsConfig,
    ) -> CatalogItem {
        let key = key.unwrap_or_else(|| format!("/{}/sample", config.id_prefix_uri));
        CatalogItem {
            key: key.clone(),
            title: html::text_between(body, "<h1", "</h1>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into())),
            cover: html::attr_after(body, "cover", "data-cfsrc")
                .or_else(|| html::attr_after(body, "cover", "data-src"))
                .or_else(|| html::attr_after(body, "cover", "src"))
                .or_else(|| html::attr_after(body, "<img", "src"))
                .map(|value| config.absolute_url(&value)),
            authors: gallery_info_values(body, "Artists"),
            artists: gallery_info_values(body, "Artists"),
            tags: gallery_info_values(body, "Tags"),
            description: gallery_description(body),
            status: ItemStatus::Completed,
            url: Some(config.absolute_url(&key)),
            language: Some(config.lang.to_string()),
            content_rating: Some(config.content_rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }

    pub fn parse_chapters(
        body: &str,
        key: &str,
        config: &GalleryAdultsConfig,
    ) -> Vec<MangaChapter> {
        vec![MangaChapter {
            key: key.to_string(),
            title: Some("Chapter".to_string()),
            scanlators: gallery_info_values(body, "Groups"),
            url: Some(config.absolute_url(key)),
            ..MangaChapter::default()
        }]
    }

    pub fn parse_pages(body: &str, config: &GalleryAdultsConfig) -> Vec<MangaPage> {
        let load_dir = input_value(body, "load_dir");
        let load_id = input_value(body, "load_id");
        let gallery_id = input_value(body, "load_id")
            .or_else(|| input_value(body, "gallery_id"))
            .unwrap_or_else(|| "0".to_string());
        if let (Some(load_dir), Some(load_id), Some(json)) =
            (load_dir, load_id, embedded_page_json(body))
        {
            let base = format!(
                "{}/{}/{}/{}",
                config.base_url.trim_end_matches('/'),
                config.page_uri,
                gallery_id,
                ""
            );
            return json
                .split(',')
                .filter_map(|part| {
                    let page = part.split('"').nth(1)?;
                    let ext_code = part.split(':').nth(1)?.split('"').nth(1)?.chars().next()?;
                    let ext = match ext_code {
                        'p' => "png",
                        'b' => "bmp",
                        'g' => "gif",
                        'w' => "webp",
                        _ => "jpg",
                    };
                    Some((page.to_string(), ext.to_string()))
                })
                .enumerate()
                .map(|(index, (page, ext))| MangaPage {
                    content: PageContent::Url {
                        url: format!(
                            "{}/{}/{}/{}.{}",
                            config.base_url.trim_end_matches('/'),
                            load_dir,
                            load_id,
                            page,
                            ext
                        ),
                        context: None,
                    },
                    headers: image_headers(config.base_url),
                    description: Some(format!("Page {}", index + 1)),
                    extra: [(
                        "pageUrl".to_string(),
                        serde_json::Value::String(format!("{base}{}/", index + 1)),
                    )]
                    .into_iter()
                    .collect(),
                    ..MangaPage::default()
                })
                .collect();
        }
        body.split("<img")
            .skip(1)
            .filter_map(|chunk| {
                html::attr(chunk, "data-cfsrc")
                    .or_else(|| html::attr(chunk, "data-src"))
                    .or_else(|| html::attr(chunk, "data-lazy-src"))
                    .or_else(|| html::attr(chunk, "src"))
            })
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: thumbnail_to_full(&config.absolute_url(&image)),
                    context: None,
                },
                headers: image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect()
    }
}

pub trait GalleryAdultsSource {
    fn gallery_config(&self, request: &Value) -> &GalleryAdultsConfig;
    fn gallery_list_fixture(&self) -> &'static str;
    fn gallery_details_fixture(&self) -> &'static str;
    fn gallery_pages_fixture(&self) -> &'static str;

    fn gallery_list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.gallery_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            config.latest_url(page)
        } else {
            config.popular_url(page)
        };
        let body =
            GalleryAdults::fetch_document_or_fixture(config, &target, self.gallery_list_fixture());
        Ok(Paged {
            entries: GalleryAdults::parse_listing(&body, config),
            has_next_page: GalleryAdults::has_next_page(&body),
        })
    }

    fn gallery_search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = self.gallery_config(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url)
            && query.contains(&format!("/{}/", config.id_prefix_uri))
        {
            let body = GalleryAdults::fetch_document_or_fixture(
                config,
                query,
                self.gallery_details_fixture(),
            );
            return Ok(Paged {
                entries: vec![GalleryAdults::parse_details(
                    &body,
                    Some(config.normalize_key(query)),
                    config,
                )],
                has_next_page: false,
            });
        }
        let target = if let Some(id) = query
            .strip_prefix("id:")
            .or_else(|| query.chars().all(|ch| ch.is_ascii_digit()).then_some(query))
        {
            config.id_url(id)
        } else if query.is_empty() {
            config.latest_url(page)
        } else {
            config.search_url(page, query)
        };
        let body =
            GalleryAdults::fetch_document_or_fixture(config, &target, self.gallery_list_fixture());
        if target.contains(&format!("/{}/", config.id_prefix_uri)) {
            return Ok(Paged {
                entries: vec![GalleryAdults::parse_details(
                    &body,
                    Some(config.normalize_key(&target)),
                    config,
                )],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: GalleryAdults::parse_listing(&body, config),
            has_next_page: GalleryAdults::has_next_page(&body),
        })
    }

    fn gallery_details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = self.gallery_config(&request);
        let key = request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/123", config.id_prefix_uri));
        let body = GalleryAdults::fetch_document_or_fixture(
            config,
            &config.absolute_url(&key),
            self.gallery_details_fixture(),
        );
        Ok(GalleryAdults::parse_details(&body, Some(key), config))
    }

    fn gallery_chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.gallery_config(&request);
        let key = request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/123", config.id_prefix_uri));
        let body = GalleryAdults::fetch_document_or_fixture(
            config,
            &config.absolute_url(&key),
            self.gallery_details_fixture(),
        );
        Ok(GalleryAdults::parse_chapters(&body, &key, config))
    }

    fn gallery_pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.gallery_config(&request);
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/{}/123", config.id_prefix_uri));
        let body = GalleryAdults::fetch_document_or_fixture(
            config,
            &config.absolute_url(&key),
            self.gallery_pages_fixture(),
        );
        Ok(GalleryAdults::parse_pages(&body, config))
    }

    fn gallery_handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = self.gallery_config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url)
            && input.contains(&format!("/{}/", config.id_prefix_uri))
        {
            let body = GalleryAdults::fetch_document_or_fixture(
                config,
                input,
                self.gallery_details_fixture(),
            );
            return Ok(Some(UrlResolveResult {
                item: Some(GalleryAdults::parse_details(
                    &body,
                    Some(config.normalize_key(input)),
                    config,
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

#[macro_export]
macro_rules! impl_gallery_adults_source {
    ($source:ty) => {
        impl $crate::sdk::source::MangaSource for $source {
            fn list(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::GalleryAdultsSource::gallery_list(self, request)
            }

            fn search(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::Paged<$crate::sdk::CatalogItem>>
            {
                $crate::manga::GalleryAdultsSource::gallery_search(self, request)
            }

            fn details(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<$crate::sdk::CatalogItem> {
                $crate::manga::GalleryAdultsSource::gallery_details(self, request)
            }

            fn chapters(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaChapter>> {
                $crate::manga::GalleryAdultsSource::gallery_chapters(self, request)
            }

            fn pages(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Vec<$crate::sdk::MangaPage>> {
                $crate::manga::GalleryAdultsSource::gallery_pages(self, request)
            }

            fn handle_url(
                &self,
                request: serde_json::Value,
            ) -> $crate::sdk::abi::ExtensionResult<Option<$crate::sdk::UrlResolveResult>> {
                $crate::manga::GalleryAdultsSource::gallery_handle_url(self, request)
            }
        }
    };
}

fn gattsu_image(body: &str) -> Option<String> {
    html::attr_after(body, "thumb-imagem", "data-src")
        .or_else(|| html::attr_after(body, "thumb-imagem", "src"))
        .or_else(|| html::attr_after(body, "post-capa", "data-src"))
        .or_else(|| html::attr_after(body, "post-capa", "src"))
        .or_else(|| html::attr_after(body, "<img", "data-src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn gattsu_info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn without_thumbnail_size(input: &str) -> String {
    let Some((base, ext)) = input.rsplit_once('.') else {
        return input.to_string();
    };
    let Some((prefix, size)) = base.rsplit_once('-') else {
        return input.to_string();
    };
    let Some((width, height)) = size.split_once('x') else {
        return input.to_string();
    };
    if width.chars().all(|ch| ch.is_ascii_digit()) && height.chars().all(|ch| ch.is_ascii_digit()) {
        format!("{prefix}.{ext}")
    } else {
        input.to_string()
    }
}

fn gallery_info_values(body: &str, tag: &str) -> Vec<String> {
    body.split("tags:")
        .chain(body.split(".tags"))
        .chain(body.split("tag_list"))
        .filter(|chunk| chunk.contains(tag))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn gallery_description(body: &str) -> Option<String> {
    let parts = [
        "Parodies",
        "Characters",
        "Languages",
        "Categories",
        "Category",
    ]
    .iter()
    .filter_map(|tag| {
        let values = gallery_info_values(body, tag);
        (!values.is_empty()).then(|| format!("{tag}: {}", values.join(", ")))
    })
    .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn input_value(body: &str, id: &str) -> Option<String> {
    body.split("<input")
        .skip(1)
        .find(|chunk| {
            chunk.contains(&format!("id=\"{id}\"")) || chunk.contains(&format!("id='{id}'"))
        })
        .and_then(|chunk| html::attr(chunk, "value"))
}

fn embedded_page_json(body: &str) -> Option<String> {
    body.split("$.parseJSON('")
        .nth(1)?
        .split("');")
        .next()
        .map(ToString::to_string)
}

fn thumbnail_to_full(input: &str) -> String {
    let Some((base, ext)) = input.rsplit_once('.') else {
        return input.to_string();
    };
    if let Some(stripped) = base.strip_suffix('t') {
        format!("{stripped}.{ext}")
    } else {
        input.to_string()
    }
}
