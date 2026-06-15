use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Hanime1 = Hanime1;
const BASE_URL: &str = "https://hanimeone.me";
const COMICS: &str = "https://hanimeone.me/comics";

struct Hanime1;

impl MangaSource for Hanime1 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{COMICS}?page={page}")
        } else {
            COMICS.to_string()
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE), &target))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let sort = request.get("filters").and_then(|f| f.get("sort")).and_then(Value::as_str).unwrap_or_default();
        let mut target = format!("{COMICS}/search?query={}&page={}", url::query_escape(query), page(&request));
        if !sort.is_empty() {
            target.push_str("&sort=");
            target.push_str(sort);
        }
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE), &target))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(parse_details(&fetch(&absolute(&key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(parse_chapters(&fetch(&absolute(&key), DETAILS_FIXTURE), &absolute(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics/sample/1".into());
        Ok(parse_pages(&fetch(&absolute(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    format!("/{}", input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/'))
}

fn absolute(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str, target: &str) -> Paged<CatalogItem> {
    let entries = body.split("comic-rows-videos-div").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&url::join_url(BASE_URL, &href));
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "comic-rows-videos-title", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Hanime1".into())),
            cover: html::attr_after(chunk, "<img", "data-srcset").or_else(|| html::attr_after(chunk, "<img", "src")).map(|v| url::join_url(BASE_URL, v.split(',').next().unwrap_or(&v).trim())),
            url: Some(absolute(&key)),
            language: Some("zh".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'") || target.contains("page=") && body.contains("pagination") }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "title comics-metadata-top-row", "</").or_else(|| html::text_between(body, "<h3", "</h3>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Hanime1.me".into()),
        cover: html::attr_after(body, "col-md-4", "data-srcset").or_else(|| html::attr_after(body, "<img", "data-srcset")).map(|v| url::join_url(BASE_URL, v.split(',').next().unwrap_or(&v).trim())),
        authors: info_value(body, "作者：").or_else(|| info_value(body, "社團：")).into_iter().collect(),
        tags: info_value(body, "分類：").map(|v| vec![v]).unwrap_or_default(),
        status: ItemStatus::Completed,
        url: Some(absolute(key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let start = body.find(label)?;
    html::text_between(&body[start..], "no-select", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str, current_url: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("comic-rows-videos-div").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let comic_url = url::join_url(BASE_URL, &href);
        let title = html::text_between(chunk, "comic-rows-videos-title", "</").map(|v| html::strip_tags(&v)).unwrap_or_else(|| "Chapter".into());
        let key = format!("{}/1", normalize_key(&comic_url).trim_end_matches('/'));
        Some(MangaChapter { key: key.clone(), title: Some(if comic_url.trim_end_matches('/') == current_url.trim_end_matches('/') { format!("當前：{title}") } else { format!("關聯：{title}") }), url: Some(absolute(&key)), ..MangaChapter::default() })
    }).collect::<Vec<_>>();
    if chapters.is_empty() {
        let key = format!("{}/1", normalize_key(current_url).trim_end_matches('/'));
        chapters.push(MangaChapter { key: key.clone(), title: Some("單章節".into()), url: Some(absolute(&key)), ..MangaChapter::default() });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let prefix = html::attr_after(body, "current-page-image", "data-prefix").unwrap_or_else(|| "https://hanimeone.me/page-".into());
    let ext = html::attr_after(body, "current-page-image", "data-extension").unwrap_or_else(|| "jpg".into());
    let count = html::attr_after(body, "comic-show-content-nav", "data-pages").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
    (0..count).map(|index| MangaPage { content: PageContent::Url { url: format!("{prefix}{}.{}", index + 1, ext), context: None }, headers: manga::image_headers(BASE_URL), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() }).collect()
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="comic-rows-videos-div"><a href="/comics/sample"><img data-srcset="/cover.jpg"><div class="comic-rows-videos-title">Sample</div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h3 class="title comics-metadata-top-row">Sample</h3><div class="col-md-4"><img data-srcset="/cover.jpg"></div>"#;
const PAGES_FIXTURE: &str = r#"<img id="current-page-image" data-prefix="https://hanimeone.me/page-" data-extension="jpg"><div class="comic-show-content-nav" data-pages="1"></div>"#;
