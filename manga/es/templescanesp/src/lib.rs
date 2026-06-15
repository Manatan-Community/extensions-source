use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: TempleScanEsp = TempleScanEsp;
const DEFAULT_BASE_URL: &str = "https://aedexnox.akan01.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const SUPABASE_URL: &str = "https://ysilhsqbtixygcgscvbb.supabase.co/rest/v1/parameters?select=value&name=eq.redirect_url_templescan";
const SUPABASE_API_KEY: &str = "sb_publishable_y5ZlqOnxowq6W7JTSZHSBQ_AQfHg77U";

struct TempleScanEsp;

impl MangaSource for TempleScanEsp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            let config = SiteConfig::new(DEFAULT_BASE_URL.to_string());
            return Ok(parse_listing(LIST_FIXTURE, &config, true));
        }
        let config = SiteConfig::from_request(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_listing(
            &fetch_document(&config, &config.list_url(page, order), LIST_FIXTURE),
            &config,
            true,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = SiteConfig::from_request(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(&config.base_url) {
            let key = config.normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&config, query, DETAILS_FIXTURE),
                    Some(key),
                    &config,
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(
            &fetch_document(&config, &config.search_url(page, query), LIST_FIXTURE),
            &config,
            true,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = SiteConfig::from_request(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(parse_details(
            &fetch_document(&config, &config.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
            &config,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = SiteConfig::from_request(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(parse_chapters(
            &fetch_document(&config, &config.absolute_url(&key), DETAILS_FIXTURE),
            &key,
            &config,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = SiteConfig::from_request(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/serie/sample/chapter-1".to_string());
        let chapter_url = config.absolute_url(&key);
        let mut body = fetch_document(&config, &chapter_url, PAGES_FIXTURE);
        if let Some(redirected) = submit_redirect_form(&body, &chapter_url, &config) {
            body = redirected;
        }
        Ok(parse_pages(&body, &config))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = SiteConfig::from_request(&request);
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = SiteConfig::from_request(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = SiteConfig::from_request(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(&config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_key(input)),
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

#[derive(Debug, Clone)]
struct SiteConfig {
    base_url: String,
}

impl SiteConfig {
    fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn from_request(request: &Value) -> Self {
        let prefs = request.get("preferences").unwrap_or(&Value::Null);
        let configured = prefs
            .get("overrideBaseUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_BASE_URL);
        let fetch_domain = prefs
            .get("fetchDomain")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if fetch_domain {
            if let Some(fetched) = fetch_current_domain(configured) {
                return Self::new(fetched);
            }
        }
        Self::new(configured.to_string())
    }

    fn absolute_url(&self, value: &str) -> String {
        url::join_url(&self.base_url, value)
    }

    fn normalize_key(&self, value: &str) -> String {
        if let Some(path) = value.strip_prefix(&self.base_url) {
            return format!("/{}", path.trim_matches('/'));
        }
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find("/serie/") {
                return format!("/{}", value[index + 1..].trim_end_matches('/'));
            }
        }
        format!("/{}", value.trim_matches('/'))
    }

    fn list_url(&self, page: u64, order: &str) -> String {
        let page_path = if page <= 1 {
            String::new()
        } else {
            format!("page/{page}/")
        };
        format!("{}/{page_path}?m_orderby={order}", self.base_url)
    }

    fn search_url(&self, page: u64, query: &str) -> String {
        let page_path = if page <= 1 {
            String::new()
        } else {
            format!("page/{page}/")
        };
        format!(
            "{}/{}?s={}&post_type=wp-manga",
            self.base_url,
            page_path,
            url::query_escape(query)
        )
    }
}

fn client(config: &SiteConfig) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", config.base_url))
        .with_cookies_for(&config.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document(config: &SiteConfig, target: &str, fixture: &str) -> String {
    client(config)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_current_domain(fallback_base: &str) -> Option<String> {
    let config = SiteConfig::new(fallback_base.to_string());
    let body = client(&config)
        .get(SUPABASE_URL)
        .header("apikey", SUPABASE_API_KEY)
        .header("Accept", "application/json")
        .send_text()
        .ok()?;
    let root = serde_json::from_str::<Value>(&body).ok()?;
    let value = root
        .as_array()?
        .first()?
        .get("value")
        .and_then(Value::as_str)?
        .trim()
        .trim_end_matches('/')
        .to_string();
    if value.is_empty() {
        None
    } else if value.starts_with("http") {
        Some(value)
    } else {
        Some(format!("https://{value}"))
    }
}

fn parse_listing(body: &str, config: &SiteConfig, paged: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("latest-poster")
                || chunk.contains("group")
                || chunk.contains("page-item-detail")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/serie/") && !href.starts_with("/serie/") {
                return None;
            }
            let key = config.normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&key))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Manga".to_string());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk).map(|value| config.absolute_url(&value)),
                url: Some(config.absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: paged && (body.contains("no-posts") == false && body.contains("loadmore")),
    }
}

fn parse_details(body: &str, key: Option<String>, config: &SiteConfig) -> CatalogItem {
    let key = key
        .map(|value| config.normalize_key(&value))
        .unwrap_or_else(|| "/serie/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_from_chunk(body).map(|value| config.absolute_url(&value)),
        description: html::text_between(body, "expand_content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: type_spans(body).into_iter().skip(1).collect(),
        status: parse_status(type_spans(body).first().map(String::as_str)),
        url: Some(config.absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str, config: &SiteConfig) -> Vec<MangaChapter> {
    let chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href") && chunk.contains("/serie/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = config.normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(config.absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "<div", "</div>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
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

fn submit_redirect_form(body: &str, chapter_url: &str, config: &SiteConfig) -> Option<String> {
    let form = body
        .split("<form")
        .skip(1)
        .find(|chunk| chunk.contains("redirect-form") && chunk.contains("method=\"post\""))?;
    let action = html::attr(form, "action")?;
    let fields = form_fields(form);
    if fields.is_empty() {
        return None;
    }
    client(config)
        .post(config.absolute_url(&action))
        .referer(chapter_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .browser_document()
        .body(form_urlencoded(&fields).into_bytes())
        .send_text()
        .ok()
}

fn parse_pages(body: &str, config: &SiteConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: manatan_extension::PageContent::Url {
                url: config.absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(&config.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "style", "style")
        .and_then(|style| {
            style.split("url(").nth(1).map(|value| {
                value
                    .split(')')
                    .next()
                    .unwrap_or(value)
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string()
            })
        })
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn type_spans(body: &str) -> Vec<String> {
    body.split("alt=type")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    let lower = value.unwrap_or_default().to_ascii_lowercase();
    if lower.contains("complet") || lower.contains("final") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("paus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn form_fields(form: &str) -> Vec<(String, String)> {
    form.split("<input")
        .skip(1)
        .filter_map(|chunk| {
            Some((
                html::attr(chunk, "name")?,
                html::attr(chunk, "value").unwrap_or_default(),
            ))
        })
        .collect()
}

fn form_urlencoded(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="latest-poster"><a href="/serie/sample"><div style="background-image:url(/cover.jpg)" class="bg-cover"></div><h3>Sample Manga</h3></a></div>
<div class="loadmore"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="wp-manga"><div class="grid"><h1>Sample Manga</h1></div><div id="expand_content">Summary.</div><div alt=type><span>En curso</span></div><div alt=type><span>Drama</span></div></div>
<img src="/cover.jpg"><ul id="list-chapters"><li><a href="/serie/sample/chapter-1"><div class="grid"><span>Chapter 1</span><div>enero 01, 2024</div></div></a></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>
"#;
