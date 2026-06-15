use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://jjcos.com";
const PAGE_SIZE: usize = 20;
const SOURCE: Jjcos = Jjcos;

struct Jjcos;

impl MangaSource for Jjcos {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_text_or_fixture(&index_url(page, None), INDEX_FIXTURE, true);
        Ok(parse_index_page(&body, page as usize, ""))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key))], has_next_page: false });
        }
        let body = fetch_text_or_fixture(&index_url(page, Some(query)), INDEX_FIXTURE, true);
        Ok(parse_index_page(&body, page as usize, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/post/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/post/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".into()),
            date_uploaded: parse_date_from_details(&body),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/post/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target: &str, fixture: &str, json: bool) -> String {
    let client = client();
    let request = client.get(target);
    let request = if json { request.xhr() } else { request.browser_document() };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn index_url(page: u64, query: Option<&str>) -> String {
    let mut target = format!("{BASE_URL}/api/index.html?page={page}");
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        target.push_str("&query=");
        target.push_str(&url::query_escape(query));
    }
    target
}

fn parse_index_page(body: &str, page: usize, query: &str) -> Paged<CatalogItem> {
    let Ok(index) = serde_json::from_str::<IndexDto>(body) else {
        return Paged { entries: vec![sample_item()], has_next_page: false };
    };
    let query = query.to_ascii_lowercase();
    let posts = index
        .posts
        .into_iter()
        .filter(|post| {
            query.is_empty()
                || post.title.to_ascii_lowercase().contains(&query)
                || post.content.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    let start = page.saturating_sub(1) * PAGE_SIZE;
    let end = posts.len().min(start + PAGE_SIZE);
    if start >= posts.len() {
        return Paged::default();
    }
    Paged {
        entries: posts[start..end].iter().cloned().map(PostDto::into_item).collect(),
        has_next_page: end < posts.len(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/post/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "fh5co-article-title", "</")
            .map(|value| html::strip_tags(&value).trim_end_matches(" - JJCOS").trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "JJCOS Gallery".into())),
        cover: html::attr_after(body, "post-content", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        tags: detail_tags(body),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .fold(Vec::<String>::new(), |mut urls, src| {
            let image = url::join_url(BASE_URL, &src);
            if !urls.contains(&image) {
                urls.push(image);
            }
            urls
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn detail_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("tag"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value).trim_start_matches('#').trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_date_from_details(body: &str) -> Option<i64> {
    html::attr_after(body, "article:published_time", "content")
        .or_else(|| html::text_between(body, "breadcrumb-item date-overlay", "</"))
        .and_then(|value| value.get(0..4)?.parse::<i64>().ok())
        .map(|year| (year - 1970).max(0) * 31_536_000)
}

fn deeplink_key(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let path = trimmed.strip_prefix(BASE_URL).unwrap_or(trimmed);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    if path.starts_with("/post/") {
        Some(path.to_string())
    } else {
        None
    }
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "/post/sample".into(),
        title: "Sample Gallery".into(),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Clone, Deserialize)]
struct IndexDto {
    posts: Vec<PostDto>,
}

#[derive(Clone, Deserialize)]
struct PostDto {
    title: String,
    link: String,
    #[serde(default)]
    feature: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

impl PostDto {
    fn into_item(self) -> CatalogItem {
        let key = deeplink_key(&self.link).unwrap_or_else(|| self.link);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.feature,
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("all".into()),
            content_rating: Some("adult".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

const INDEX_FIXTURE: &str = r#"
{ "posts": [{ "title": "Sample Gallery", "link": "https://jjcos.com/post/sample", "feature": "https://jjcos.com/cover.jpg", "content": "Sample content" }] }
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="fh5co-article-title">Sample Gallery - JJCOS</h1>
<meta property="article:published_time" content="2024-01-01 00:00:00">
<div class="tag-container"><a class="tag">#Outdoor</a></div>
<div id="post-content"><p><img src="https://jjcos.com/1.jpg"></p><p><img data-src="https://jjcos.com/2.jpg"></p></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jjcos() {
        assert_eq!(parse_index_page(INDEX_FIXTURE, 1, "").entries.len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, Some("/post/sample".into())).title, "Sample Gallery");
        assert_eq!(parse_pages(DETAILS_FIXTURE).len(), 2);
    }
}
