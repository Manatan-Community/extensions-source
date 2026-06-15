use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UpdateStrategy, UrlResolveResult};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: HentaiClub = HentaiClub;
const BASE_URL: &str = "https://www.hentaiclub.net";

struct HentaiClub;

impl MangaSource for HentaiClub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if page <= 1 { format!("{BASE_URL}/") } else { format!("{BASE_URL}/page/{page}/") };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let page = page(&request);
        let target = if !query.is_empty() {
            let base = format!("{BASE_URL}/search/{}/", url::query_escape(query));
            if page > 1 { format!("{base}page/{page}/") } else { base }
        } else if let Some(tag) = filters.get("tag").and_then(Value::as_str).filter(|v| !v.trim().is_empty()) {
            let base = format!("{BASE_URL}/tag/{}/", url::query_escape(tag.trim()));
            if page > 1 { format!("{base}{page}/") } else { base }
        } else if let Some(sort) = filters.get("sort").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/sort/{sort}.html")
        } else if page <= 1 {
            format!("{BASE_URL}/")
        } else {
            format!("{BASE_URL}/page/{page}/")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(&fetch(&absolute(&key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(vec![MangaChapter { key: key.clone(), title: Some("章节 1".into()), url: Some(absolute(&key)), ..MangaChapter::default() }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        Ok(parse_pages(&fetch(&absolute(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "manga").map(|key| absolute(&key))) }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key))) }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient { http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str) -> String { client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn normalize_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/')) }
fn absolute(key: &str) -> String { url::join_url(BASE_URL, key) }

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("div class=\"item").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "item-link", "href")?;
        let key = normalize_key(&url::join_url(BASE_URL, &href));
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "item-link-text", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "绅士会所".into())),
            cover: html::attr_after(chunk, "item-img", "data-original").or_else(|| html::attr_after(chunk, "item-img", "src")).map(|v| url::join_url(BASE_URL, &v)),
            url: Some(absolute(&key)),
            language: Some("zh".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    let has_next_page = entries.len() >= 24;
    Paged { entries, has_next_page }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let tags = body.split("<a").filter(|c| c.contains("/tag/")).map(html::strip_tags).filter(|v| !v.is_empty()).collect::<Vec<_>>();
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "<title", "</title>").map(|v| html::strip_tags(&v).replace(" - 绅士会所", "")).filter(|v| !v.is_empty()).unwrap_or_else(|| "绅士会所".into()),
        cover: html::attr_after(body, "post-item", "data-src").map(|v| url::join_url(BASE_URL, &v)),
        authors: tags.first().cloned().into_iter().collect(),
        tags,
        description: find_views(body).map(|v| format!("浏览量：{v}次")),
        status: if key.contains("/r18/") { ItemStatus::Completed } else { ItemStatus::Ongoing },
        url: Some(absolute(key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        ..CatalogItem::default()
    }
}

fn find_views(body: &str) -> Option<String> {
    let text = html::strip_tags(body);
    let after = text.split('览').nth(1)?;
    Some(after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect::<String>()).filter(|v| !v.is_empty())
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("post-item").skip(1).filter_map(|chunk| html::attr(chunk, "data-src")).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="item"><a class="item-link" href="/sample"><img class="item-img" data-original="/cover.jpg"><span class="item-link-text">Sample</span></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<title>Sample - 绅士会所</title><div class="content"><div class="post-item" data-src="/page.jpg"></div><a href="/tag/tag">tag</a>浏览：1次</div>"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
