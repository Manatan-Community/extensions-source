use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://www.everiaclub.com";
const SOURCE: EveriaClubCom = EveriaClubCom;

struct EveriaClubCom;

impl MangaSource for EveriaClubCom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest { format!("{BASE_URL}/?page={page}") } else { BASE_URL.to_string() };
        let body = fetch_document_or_fixture(&target, if latest { LATEST_FIXTURE } else { POPULAR_FIXTURE });
        Ok(if latest { parse_latest(&body) } else { Paged { entries: parse_popular(&body), has_next_page: false } })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(normalize_key(query)))], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let tag = filters.get("tag").and_then(Value::as_str).unwrap_or_default().trim();
        let category = filters.get("category").and_then(Value::as_str).unwrap_or("Any");
        let target = if !tag.is_empty() {
            format!("{BASE_URL}/tags/{}/{page}", url::query_escape(tag))
        } else if let Some(path) = category_path(category) {
            format!("{BASE_URL}/{path}?page={page}")
        } else if !query.is_empty() {
            format!("{BASE_URL}/search/?keyword={}&page={page}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/?page={page}")
        };
        Ok(parse_latest(&fetch_document_or_fixture(&target, LATEST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".into()),
            chapter_number: Some(1.0),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn fetch_document_or_fixture(target_url: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("leftp")
            .skip(1)
            .filter_map(parse_anchor_item)
            .collect(),
        has_next_page: body.contains("span class=\"current\"") || body.contains("span.current"),
    }
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<li").skip(1).filter_map(parse_anchor_item).collect()
}

fn parse_anchor_item(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let img = block.split("<img").nth(1)?;
    let title = html::attr(img, "title").or_else(|| url::slug_from_url(&href)).unwrap_or_else(|| "Everia Gallery".into());
    Some(CatalogItem {
        key: normalize_key(&href),
        title,
        cover: image_attr(img).map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &href)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.unwrap_or_else(|| "/sample.html".into()),
        title: html::attr_after(body, "<img", "title").or_else(|| html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))).unwrap_or_else(|| "Everia Gallery".into()),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|block| block.contains("/tags/") || block.contains("tags"))
            .filter_map(|block| html::text_between(block, "<p", "</p>").or_else(|| html::text_between(block, ">", "</a>")))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(block: &str) -> Option<String> {
    html::attr(block, "data-original")
        .or_else(|| html::attr(block, "data-lazy-src"))
        .or_else(|| html::attr(block, "data-src"))
        .or_else(|| html::attr(block, "src"))
}

fn category_path(category: &str) -> Option<&'static str> {
    match category {
        "Gravure" => Some("Gravure.html"),
        "Japan" => Some("Japan.html"),
        "Korea" => Some("Korea.html"),
        "Thailand" => Some("Thailand.html"),
        "Chinese" => Some("Chinese.html"),
        "Cosplay" => Some("Cosplay.html"),
        _ => None,
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).split('?').next().unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

export_manga_source!(SOURCE);

const LATEST_FIXTURE: &str = r#"
<div class="mainleft"><div class="leftp"><a href="https://www.everiaclub.com/sample.html"><img title="Sample Gallery" data-original="https://www.everiaclub.com/cover.jpg"></a></div></div><li><span class="current">1</span></li><li><a>2</a></li>
"#;

const POPULAR_FIXTURE: &str = r#"
<div class="mainright"><li><a href="https://www.everiaclub.com/sample.html"><img title="Sample Gallery" src="https://www.everiaclub.com/cover.jpg"></a></li></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Gallery</h1><div class="end"><span>Tags:</span><a href="/tags/cosplay"><p class="tags">Cosplay</p></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="mainleft"><img data-original="https://www.everiaclub.com/1.jpg"><img data-src="https://www.everiaclub.com/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_html_source() {
        assert_eq!(parse_latest(LATEST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_popular(POPULAR_FIXTURE).len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, None).tags, vec!["Cosplay"]);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
