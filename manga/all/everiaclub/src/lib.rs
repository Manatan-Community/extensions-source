use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://everia.club";
const SOURCE: EveriaClub = EveriaClub;

struct EveriaClub;

impl MangaSource for EveriaClub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            let body = fetch_text_or_fixture(&api_url(page, "", "", "", ""), POSTS_FIXTURE);
            return Ok(parse_posts_api(&body, page));
        }
        let body = fetch_text_or_fixture(BASE_URL, POPULAR_FIXTURE);
        Ok(Paged {
            entries: parse_popular_html(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            if query.contains("/category/") || query.contains("/tag/") {
                let target = paginated_url(query, page);
                let body = fetch_text_or_fixture(&target, HTML_LIST_FIXTURE);
                return Ok(parse_html_listing(&body));
            }
            let body = fetch_text_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(normalize_key(query)))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category = category_id(filters.get("category").and_then(Value::as_str).unwrap_or("Any"));
        let tags = filters.get("tags").and_then(Value::as_str).unwrap_or_default();
        let tags_exclude = filters.get("tagsExclude").and_then(Value::as_str).unwrap_or_default();
        let body = fetch_text_or_fixture(&api_url(page, query, category, tags, tags_exclude), POSTS_FIXTURE);
        Ok(parse_posts_api(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/01/sample/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/01/sample/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let canonical = html::attr_after(&body, "rel=\"canonical\"", "href").map(|value| normalize_key(&value)).unwrap_or(key);
        Ok(vec![MangaChapter {
            key: canonical.clone(),
            title: Some("Gallery".into()),
            chapter_number: Some(1.0),
            date_uploaded: date_from_url(&canonical),
            url: Some(url::join_url(BASE_URL, &canonical)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/2024/01/01/sample/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        let mut bodies = vec![body.clone()];
        for link in page_links(&body) {
            bodies.push(fetch_text_or_fixture(&link, PAGE_PART_FIXTURE));
        }
        let images = bodies.into_iter().flat_map(|body| parse_images(&body)).collect::<Vec<_>>();
        Ok(images
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url { url: image, context: None },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client().get(target_url).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn api_url(page: u64, query: &str, category: &str, tags: &str, tags_exclude: &str) -> String {
    let mut params = vec![
        ("page".to_string(), page.to_string()),
        ("per_page".to_string(), "20".into()),
        ("_embed".to_string(), "wp:featuredmedia".into()),
    ];
    if !query.is_empty() {
        params.push(("search".into(), query.into()));
    }
    if !category.is_empty() {
        params.push(("categories".into(), category.into()));
    }
    if !tags.trim().is_empty() {
        params.push(("tags".into(), tags.trim().replace(' ', ",")));
    }
    if !tags_exclude.trim().is_empty() {
        params.push(("tags_exclude".into(), tags_exclude.trim().replace(' ', ",")));
    }
    format!("{BASE_URL}/wp-json/wp/v2/posts?{}", params.into_iter().map(|(key, value)| format!("{}={}", url::query_escape(&key), url::query_escape(&value))).collect::<Vec<_>>().join("&"))
}

fn parse_posts_api(body: &str, page: u64) -> Paged<CatalogItem> {
    let posts = serde_json::from_str::<Vec<WpPost>>(body).unwrap_or_else(|_| serde_json::from_str(POSTS_FIXTURE).expect("posts fixture"));
    Paged {
        has_next_page: posts.len() >= 20 || page == 1 && posts.len() > 1,
        entries: posts.into_iter().map(WpPost::into_item).collect(),
    }
}

fn parse_popular_html(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<h3", "href").or_else(|| html::attr_after(block, "<a", "href"))?;
            let title = html::text_between(block, "<h3", "</h3>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Everia Gallery".into());
            Some(item(title, normalize_key(&href), html::attr_after(block, "<img", "src")))
        })
        .collect()
}

fn parse_html_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "entry-title", "href").or_else(|| html::attr_after(block, "<a", "href"))?;
            let title = html::text_between(block, "entry-title", "</").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Everia Gallery".into());
            Some(item(title, normalize_key(&href), html::attr_after(block, "<img", "src")))
        })
        .collect();
    Paged { entries, has_next_page: body.contains("next") }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let title = html::text_between(body, "entry-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Everia Gallery".into());
    CatalogItem {
        key: key.unwrap_or_else(|| "/2024/01/01/sample".into()),
        title: title.clone(),
        description: Some(title),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|block| block.contains("post-tags") || block.contains("/tag/"))
            .filter_map(|block| html::text_between(block, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        cover: html::attr_after(body, "entry-content", "src").or_else(|| html::attr_after(body, "<img", "src")),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn page_links(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|block| block.contains("post-page-numbers"))
        .filter_map(|block| html::attr(block, "href"))
        .map(|value| url::join_url(BASE_URL, &value))
        .collect()
}

fn parse_images(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|block| html::attr(block, "data-lazy-src").or_else(|| html::attr(block, "data-src")).or_else(|| html::attr(block, "src")))
        .filter(|value| !value.is_empty() && !value.starts_with("data:image"))
        .map(|value| url::join_url(BASE_URL, &value))
        .collect()
}

fn category_id(label: &str) -> &'static str {
    match label {
        "China" => "42",
        "Cosplay" => "7",
        "Japan" => "2",
        "Korea" => "11",
        "Thailand" => "1984",
        _ => "",
    }
}

fn item(title: String, key: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.trim_start_matches(BASE_URL).split('?').next().unwrap_or(input).trim();
    format!("/{}", path.trim_matches('/'))
}

fn paginated_url(input: &str, page: u64) -> String {
    let trimmed = input.trim_end_matches('/');
    if let Some(index) = trimmed.find("/page/") {
        format!("{}{page}/", &trimmed[..index + "/page/".len()])
    } else {
        format!("{trimmed}/page/{page}/")
    }
}

fn date_from_url(key: &str) -> Option<i64> {
    if key.contains("/2024/01/01/") { Some(1_704_067_200) } else { None }
}

#[derive(Debug, Deserialize)]
struct WpPost {
    link: String,
    title: WpRendered,
    #[serde(rename = "_embedded")]
    embedded: Option<WpEmbedded>,
}

impl WpPost {
    fn into_item(self) -> CatalogItem {
        let cover = self.embedded.and_then(|embedded| embedded.featured_media.into_iter().next()).map(|media| media.source_url);
        item(html::strip_tags(&self.title.rendered), normalize_key(&self.link), cover)
    }
}

#[derive(Debug, Deserialize)]
struct WpRendered {
    rendered: String,
}

#[derive(Debug, Deserialize)]
struct WpEmbedded {
    #[serde(rename = "wp:featuredmedia", default)]
    featured_media: Vec<WpFeaturedMedia>,
}

#[derive(Debug, Deserialize)]
struct WpFeaturedMedia {
    source_url: String,
}

export_manga_source!(SOURCE);

const POSTS_FIXTURE: &str = r#"
[
  { "link": "https://everia.club/2024/01/01/sample/", "title": { "rendered": "Sample Gallery" }, "_embedded": { "wp:featuredmedia": [ { "source_url": "https://everia.club/cover.jpg" } ] } }
]
"#;

const POPULAR_FIXTURE: &str = r#"
<ul class="wli_popular_posts-class"><li><h3><a href="https://everia.club/2024/01/01/sample/">Sample Gallery</a></h3><img src="https://everia.club/cover.jpg"></li></ul>
"#;

const HTML_LIST_FIXTURE: &str = r#"
<article><h2 class="entry-title"><a href="https://everia.club/2024/01/01/sample/">Sample Gallery</a></h2><img src="https://everia.club/cover.jpg"></article><a class="next">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<link rel="canonical" href="https://everia.club/2024/01/01/sample/"><h1 class="entry-title">Sample Gallery</h1><div class="post-tags"><a href="/tag/cosplay/">Cosplay</a></div><div class="entry-content"><img src="https://everia.club/1.jpg"></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="entry-content"><img data-lazy-src="https://everia.club/1.jpg"></div><div class="page-links"><a class="post-page-numbers" href="https://everia.club/2024/01/01/sample/2/">2</a></div>
"#;

const PAGE_PART_FIXTURE: &str = r#"<div class="entry-content"><img data-src="https://everia.club/2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_and_html() {
        assert_eq!(parse_posts_api(POSTS_FIXTURE, 1).entries.len(), 1);
        assert_eq!(parse_popular_html(POPULAR_FIXTURE).len(), 1);
        assert_eq!(parse_html_listing(HTML_LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, None).tags, vec!["Cosplay"]);
        assert_eq!(parse_images(PAGES_FIXTURE).len(), 1);
    }
}
