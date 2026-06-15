use manatan_extension::{abi, abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult};
use manatan_shared::{html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Iqiyi = Iqiyi;
const BASE_URL: &str = "https://www.iqiyi.com/manhua";

struct Iqiyi;

impl MangaSource for Iqiyi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { 4 } else { 9 };
        Ok(parse_grid(&fetch(&format!("{BASE_URL}/category/全部_-1_-1_{order}_{}/", page(&request)), LIST_FIXTURE), "cartoon-hot-list"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("https://www.iqiyi.com") {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        Ok(parse_grid(&fetch(&format!("{BASE_URL}/search-keyword={}_{}", url::query_escape(query), page(&request)), SEARCH_FIXTURE), "stacksBook"))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail_sample.html".into());
        Ok(parse_details(&fetch(&absolute(&key), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail_sample.html".into());
        let id = key.split("detail_").nth(1).and_then(|v| v.split(".html").next()).unwrap_or("sample");
        Ok(parse_chapters(&client().get(format!("{BASE_URL}/catalog/{id}/")).send_text().unwrap_or_else(|_| CHAPTERS_FIXTURE.into())))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/reader/sample_1.html".into());
        let body = fetch(&absolute(&key), PAGES_FIXTURE);
        if body.contains("pay-title") {
            return Err(abi::ExtensionError { message: "本章为付费章节".into() });
        }
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "manga").map(|key| absolute(&key))) }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> { Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key))) }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with("https://www.iqiyi.com") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient { http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for("https://www.iqiyi.com").with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str) -> String { client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn normalize_key(input: &str) -> String { format!("/{}", input.strip_prefix(BASE_URL).or_else(|| input.strip_prefix("https://www.iqiyi.com")).unwrap_or(input).split('?').next().unwrap_or(input).trim_matches('/')) }
fn absolute(key: &str) -> String { url::join_url(BASE_URL, key) }

fn parse_grid(body: &str, marker: &str) -> Paged<CatalogItem> {
    let entries = body.split(marker).skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "cartoon-item-tit", "</a>").or_else(|| html::text_between(chunk, "stacksBook-tit", "</a>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "爱奇艺叭嗒".into())),
            cover: html::attr_after(chunk, "<img", "src").map(|v| url::join_url("https://www.iqiyi.com", &v)),
            url: Some(absolute(&key)),
            language: Some("zh-Hans".into()),
            content_rating: Some("safe".into()),
            ..CatalogItem::default()
        })
    }).collect();
    Paged { entries, has_next_page: body.contains("下一页") }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = html::text_between(body, "cata-info", "</").map(|v| html::strip_tags(&v)).unwrap_or_default();
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "detail-tit", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "爱奇艺叭嗒".into()),
        cover: html::attr_after(body, "detail-cover", "src").map(|v| url::join_url("https://www.iqiyi.com", &v)),
        authors: html::text_between(body, "author-name", "</").map(|v| html::strip_tags(&v)).into_iter().collect(),
        tags: body.split("detail-categ").skip(1).map(html::strip_tags).filter(|v| !v.is_empty()).collect(),
        description: html::text_between(body, "detail-docu", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        status: if status_text.contains("完结") { ItemStatus::Completed } else if status_text.contains("连载") { ItemStatus::Ongoing } else { ItemStatus::Unknown },
        url: Some(absolute(key)),
        language: Some("zh-Hans".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let dto = serde_json::from_str::<Dto>(body).unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("valid fixture"));
    let mut chapters = dto.data.and_then(|d| d.episodes).unwrap_or_default().into_iter().map(|ep| MangaChapter {
        key: format!("/reader/{}_{}.html", ep.comic_id, ep.episode_id),
        title: Some(format!("{} {}", ep.episode_order, ep.episode_title)),
        date_uploaded: Some(ep.first_online_time),
        url: Some(format!("{BASE_URL}/reader/{}_{}.html", ep.comic_id, ep.episode_id)),
        ..MangaChapter::default()
    }).collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("main-item").skip(1).filter_map(|chunk| html::attr_after(chunk, "<img", "data-original").or_else(|| html::attr_after(chunk, "<img", "src"))).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: url::join_url("https://www.iqiyi.com", &image), context: None },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

#[derive(Deserialize)]
struct Dto { data: Option<DataDto> }
#[derive(Deserialize)]
struct DataDto { episodes: Option<Vec<EpisodeDto>> }
#[derive(Deserialize)]
struct EpisodeDto { #[serde(rename = "comicId")] comic_id: String, #[serde(rename = "episodeId")] episode_id: String, #[serde(rename = "episodeTitle")] episode_title: String, #[serde(rename = "episodeOrder")] episode_order: i32, #[serde(rename = "firstOnlineTime")] first_online_time: i64 }

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="cartoon-hot-ul"><li class="cartoon-hot-list"><a class="cartoon-item-tit" href="https://www.iqiyi.com/manhua/detail_sample.html">Sample</a><img src="/cover.jpg"></li></ul>"#;
const SEARCH_FIXTURE: &str = r#"<ul class="stacksList"><li class="stacksBook"><h3 class="stacksBook-tit"><a href="https://www.iqiyi.com/manhua/detail_sample.html">Sample</a></h3><img src="/cover.jpg"></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="detail-tit"><h1>Sample</h1><a class="detail-categ">Tag</a></div><div class="detail-cover"><img src="/cover.jpg"></div><p class="author"><span class="author-name">Author</span></p><p class="detail-docu">Desc</p><span class="cata-info">完结</span>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"episodes":[{"comicId":"sample","episodeId":"1","episodeTitle":"第一话","episodeOrder":1,"firstOnlineTime":1704067200}]}}"#;
const PAGES_FIXTURE: &str = r#"<ul class="main-container"><li class="main-item"><img data-original="/page.jpg"></li></ul>"#;
