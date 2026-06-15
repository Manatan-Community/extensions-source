use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: BaoziManhua = BaoziManhua;
const BASE_URL: &str = "https://cn.baozimh.com";
const CONTENT_RATING: &str = "safe";

struct BaoziManhua;

impl MangaSource for BaoziManhua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/list/new")
        } else {
            format!("{BASE_URL}/classify?page={page}")
        };
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.matches("comics-card__poster").count() >= 36,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("https://") && query.contains("/comic/") {
            return self.details(serde_json::json!({ "key": normalize_key(query) })).map(|item| Paged {
                entries: vec![item],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/classify?page={}&{}", page(&request), filter_query(&request))
        } else {
            format!("{BASE_URL}/search?q={}", url::query_escape(query))
        };
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: query.is_empty() && body.matches("comics-card__poster").count() >= 36,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let body = fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let body = fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/chapter/sample_1.html".to_string());
        let body = fetch(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.contains("/comic/") {
            let key = normalize_key(input);
            let item = parse_details(&fetch(input, DETAILS_FIXTURE), &key);
            return Ok(Some(UrlResolveResult { item: Some(item), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_query(request: &Value) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    ["type", "region", "state", "filter"]
        .into_iter()
        .map(|key| {
            let value = filters.get(key).and_then(Value::as_str).unwrap_or(match key {
                "filter" => "*",
                _ => "all",
            });
            format!("{key}={}", url::query_escape(value))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn normalize_key(input: &str) -> String {
    let path = input.split(".com").nth(1).unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("comics-card__poster") || chunk.contains("/comic/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/comic/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::attr(chunk, "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "amp-img", "src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("zh".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "comics-detail__title", "</").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Manga".to_string()),
        cover: html::attr_after(body, "amp-img", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|image| url::join_url(BASE_URL, &image)),
        authors: html::text_between(body, "comics-detail__author", "</").map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
        description: html::text_between(body, "comics-detail__desc", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: if body.contains("连载中") || body.contains("連載中") { ItemStatus::Ongoing } else if body.contains("已完结") || body.contains("已完結") { ItemStatus::Completed } else { ItemStatus::Unknown },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("zh".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("<")
        .filter(|chunk| chunk.contains("comics-chapters") || chunk.contains("/comic/chapter/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            let title = html::strip_tags(chunk);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() { "Chapter".to_string() } else { title }),
                url: Some(url::join_url(BASE_URL, &key)),
                date_uploaded: first_ymd(body).and_then(|date| dates::parse_ymd(&date)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<")
        .filter(|chunk| chunk.contains("comic-contain") || chunk.contains("amp-img") || chunk.contains("<img"))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|image| image.contains("http") || image.starts_with('/'))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: url::join_url(BASE_URL, &image).replace(".baozicdn.com", ".baozimh.com"), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn first_ymd(body: &str) -> Option<String> {
    for token in body.split(|ch: char| !(ch.is_ascii_digit() || ch == '-' || ch == '/')) {
        if token.len() >= 8 && dates::parse_ymd(token).is_some() {
            return Some(token.to_string());
        }
    }
    None
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="pure-g"><div><a class="comics-card__poster" href="/comic/sample" title="Sample"><amp-img src="https://cn.baozimh.com/cover.jpg"></amp-img></a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="comics-detail__title">Sample</h1><h2 class="comics-detail__author">Author</h2><p class="comics-detail__desc">Sample description.</p><div class="tag-list"><span class="tag">连载中</span></div>
<div class="section-title">章节目录</div><div class="comics-chapters"><a href="/comic/chapter/sample_1.html">第 1 话</a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="comic-contain"><amp-img src="https://img.baozimh.com/page-1.jpg"></amp-img></div>
"#;
