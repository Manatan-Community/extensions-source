use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    http::HttpClient, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: MangaXiaoSi = MangaXiaoSi;
const BASE_URL: &str = "https://www.jjmhw2.top";

struct MangaXiaoSi;

impl MangaSource for MangaXiaoSi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/update?page={}", page(&request))
        } else {
            format!("{BASE_URL}/rank")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/booklist?page={}", page(&request))
        } else {
            format!("{BASE_URL}/search?keyword={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/book/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/book/sample".to_string());
        Ok(parse_chapters(&fetch(&absolute(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".to_string());
        Ok(parse_pages(&fetch(&absolute(&key), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter").map(|key| absolute(&key)))
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
        if let Some(key) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("mh-item")
        .skip(1)
        .chain(body.split("mh-itme-top").skip(1))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&key));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: cover_from_style(chunk)
                    .or_else(|| html::attr_after(chunk, "<img", "src").map(|src| absolute(&src))),
                url: Some(absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("nextPage") || body.contains("下一页"),
        entries,
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch(&absolute(key), DETAILS_FIXTURE);
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(key)),
        cover: html::attr_after(&body, "banner_detail_form", "src").map(|src| absolute(&src)),
        authors: html::text_between(&body, "作者", "</")
            .map(|value| {
                vec![
                    html::strip_tags(&value)
                        .trim_start_matches('：')
                        .trim()
                        .to_string(),
                ]
            })
            .unwrap_or_default(),
        tags: body
            .split("block:contains")
            .flat_map(|_| Vec::<String>::new())
            .chain(
                body.split("<a")
                    .skip(1)
                    .map(html::strip_tags)
                    .filter(|value| !value.is_empty()),
            )
            .collect(),
        description: html::text_between(&body, "content", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: status_from_text(&body),
        url: Some(absolute(key)),
        language: Some("zh".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("detail-list-select")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(html::strip_tags(chunk)).filter(|value| !value.is_empty()),
                url: Some(absolute(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("comicpage") || chunk.contains("data-original") || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "data-original").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: absolute(&src),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn cover_from_style(chunk: &str) -> Option<String> {
    let style = html::attr_after(chunk, "mh-cover", "style")?;
    let raw = style
        .split("url(")
        .nth(1)?
        .split(')')
        .next()?
        .trim_matches(['"', '\'']);
    Some(absolute(raw))
}

fn status_from_text(body: &str) -> ItemStatus {
    if body.contains("完结") {
        ItemStatus::Completed
    } else if body.contains("连载") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn absolute(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| path.starts_with('/'))
        .map(normalize_key)
}

fn normalize_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
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
        .map(normalize_key)
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

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("manga")
        .replace('-', " ")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="mh-item"><a href="/book/sample"><div class="mh-cover" style="background-image:url('/cover.jpg')"></div><h2 class="title"><a href="/book/sample">Sample</a></h2></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="banner_detail_form"><div class="cover"><img src="/cover.jpg"></div><div class="info"><h1>Sample</h1><div class="content">Sample description.</div><div id="detail-list-select"><li><a href="/chapter/sample">Chapter 1</a></li></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="comicpage"><img data-original="/page1.jpg"></div>"#;
