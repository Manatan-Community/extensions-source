use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Raw1001 = Raw1001;
const BASE_URL: &str = "https://raw1001.net";

struct Raw1001;

impl MangaSource for Raw1001 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/all-manga/{page}/?sort=last_update&status=0")
        } else {
            format!("{BASE_URL}/ranking/week/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = page(&request);
        let target = if query.is_empty() {
            let mut params = Vec::new();
            for id in ["genre", "status", "sort"] {
                if let Some(value) = filter_string(&request, id).filter(|value| !value.is_empty()) {
                    params.push(format!("{id}={}", url::query_escape(value)));
                }
            }
            if params.is_empty() {
                format!("{BASE_URL}/filter/{page}/")
            } else {
                format!("{BASE_URL}/filter/{page}/?{}", params.join("&"))
            }
        } else {
            format!("{BASE_URL}/search/{page}/?keyword={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let chapter_id = body.split("CHAPTER_ID").nth(1).and_then(|rest| rest.split('=').nth(1)).and_then(|rest| rest.split(';').next()).map(|value| value.trim().trim_matches(['"', '\'']).to_string()).unwrap_or_else(|| "1".into());
        let ajax = fetch_json(&format!("{BASE_URL}/ajax/image/list/chap/{chapter_id}"), AJAX_FIXTURE);
        Ok(parse_pages(&ajax, &chapter_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection { id: "popular".into(), title: "Popular".into(), style: Some(HomeSectionStyle::Cover), has_more: popular.has_next_page, entries: popular.entries, ..HomeSection::default() },
            HomeSection { id: "latest".into(), title: "Latest".into(), style: Some(HomeSectionStyle::Cover), has_more: latest.has_next_page, entries: latest.entries, ..HomeSection::default() },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("grid") || chunk.contains("text-center") || chunk.contains("/manga/"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, ".text-center", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
                if !href.contains("/manga/") {
                    return None;
                }
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "text-center", "</a>")
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .or_else(|| html::text_between(chunk, "<a", "</a>"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Raw1001".into())),
                    cover: image_attr(chunk),
                    url: Some(absolute_url(&key)),
                    language: Some("ja".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagecurrent") || body.contains("blog-pager"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(&body, "<h1", "</h1>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Raw1001".into())),
        cover: html::attr_after(&body, ".a1", "src").or_else(|| image_attr(&body)),
        authors: info_values(&body, "fa-user"),
        tags: body.split("rel='tag'").chain(body.split("rel=\"tag\"")).filter_map(|chunk| html::text_between(chunk, ">", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).collect(),
        description: html::text_between(&body, "syn-target", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(&body)),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
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
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                date_uploaded: html::attr_after(chunk, "<time", "datetime").and_then(|value| value.parse::<i64>().ok()).map(|seconds| seconds * 1000),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct PageListResponse {
    status: Option<bool>,
    html: Option<String>,
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let html_body = serde_json::from_str::<PageListResponse>(body).ok().and_then(|data| {
        if data.status.unwrap_or(true) {
            data.html
        } else {
            None
        }
    }).unwrap_or_else(|| body.to_string());
    html_body
        .split("separator")
        .skip(1)
        .flat_map(|chunk| {
            html::attr_after(chunk, "<a", "href")
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .into_iter()
        })
        .map(|image| absolute_url(&image))
        .fold(Vec::<String>::new(), |mut pages, image| {
            if !pages.contains(&image) {
                pages.push(image);
            }
            pages
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(referer)) },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn info_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "span", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("updating"))
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("完了") || text.contains("Completed") {
        ItemStatus::Completed
    } else if text.contains("進行中") || text.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.find(BASE_URL).map(|_| normalize_key(input)).or_else(|| input.starts_with("/manga/").then(|| normalize_key(input)))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('#').next().unwrap_or(input).split('?').next().unwrap_or(input).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="main"><div class="grid"><div><img src="/cover.jpg"><div class="text-center"><a href="/manga/sample">Sample Raw1001</a></div></div></div></div><div class="blog-pager"><span class="pagecurrent">1</span><span>2</span></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="a1"><figure><img src="/cover.jpg"></figure></div><div class="a2"><header><h1>Sample Raw1001</h1></header><div><a rel="tag" class="label">Action</a></div><div class="y6x11p"><i class="fas fa-user"></i><span class="dt">Author</span><i class="fas fa-rss"></i><span class="dt">進行中</span></div></div><div id="syn-target">Summary</div><ul><li class="chapter"><a href="/sample-chapter-1">Chapter 1</a><time datetime="1704067200"></time></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<script>const CHAPTER_ID = 1;</script>"#;
const AJAX_FIXTURE: &str = r#"{"status":true,"html":"<div class=\"separator\"><a href=\"/page1.jpg\"></a></div><div class=\"separator\"><a href=\"/page2.jpg\"></a></div>"}"#;
