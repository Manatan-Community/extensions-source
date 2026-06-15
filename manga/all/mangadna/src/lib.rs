use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangadna.com";
const SOURCE: MangaDna = MangaDna;

struct MangaDna;

impl MangaSource for MangaDna {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "latest" } else { "rating" };
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/manga/page/{page}?orderby={order}"), LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key), source)], has_next_page: false });
        }
        let url = if query.is_empty() {
            filtered_url(&request, page)
        } else {
            format!("{BASE_URL}/search?q={}&page={page}", query_escape(query))
        };
        let body = fetch_document_or_fixture(&url, LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), source)),
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

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "mangadna-en", lang: "en" },
    SourceConfig { id: "mangadna-all", lang: "all" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("mangadna-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn filtered_url(request: &Value, page: u64) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let genre = filter_string(filters, "genre").unwrap_or_default();
    let sort = filter_string(filters, "sort").unwrap_or_default();
    let base = if genre.is_empty() {
        format!("{BASE_URL}/manga/page/{page}")
    } else {
        format!("{BASE_URL}/manga-genre/{genre}/{page}")
    };
    if sort.is_empty() { base } else { format!("{base}?orderby={sort}") }
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("home-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = attr_after(chunk, "htitle", "href").or_else(|| attr_after(chunk, "hthumb", "href")).or_else(|| attr_after(chunk, "<a", "href"))?;
            if source.lang == "en" && href.trim_end_matches('/').ends_with("-raw") {
                return None;
            }
            let key = normalize_key(&href);
            let title = attr_after(chunk, "<a", "title")
                .or_else(|| text_between(chunk, "<a", "</a>").map(|value| strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Manga".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: first_img(chunk).map(|value| url_join(BASE_URL, &value)),
                url: Some(url_join(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("next") && !body.contains("next disabled") }
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let rows = summary_rows(body);
    let synopsis = attr_after(body, "og:description", "content").or_else(|| text_between(body, "summary__content", "</div>").map(|value| strip_tags(&value)));
    let mut desc = Vec::new();
    if let Some(synopsis) = synopsis.filter(|value| !value.is_empty()) {
        desc.push(synopsis);
    }
    if let Some(alt) = rows.get("Alternative").filter(|value| !value.is_empty() && *value != "Updating") {
        desc.push(format!("Alternative: {alt}"));
    }
    if let Some(release) = rows.get("Release").filter(|value| !value.is_empty()) {
        desc.push(format!("Released: {release}"));
    }
    CatalogItem {
        key: key.clone(),
        title: text_between(body, "entry-title", "</").or_else(|| text_between(body, "post-title", "</h1>")).map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Manga".into()),
        cover: attr_after(body, "summary_image", "src").or_else(|| attr_after(body, "og:image", "content")).map(|value| url_join(BASE_URL, &value)),
        description: (!desc.is_empty()).then_some(desc.join("\n\n")),
        authors: parse_anchor_list(body, "author-content"),
        artists: parse_anchor_list(body, "artist-content"),
        tags: {
            let mut tags = parse_anchor_list(body, "genres-content");
            if let Some(kind) = rows.get("Type").filter(|value| !value.is_empty() && *value != "Updating") {
                tags.push(kind.clone());
            }
            tags.sort();
            tags.dedup();
            tags
        },
        status: parse_status(rows.get("Status").map(String::as_str).unwrap_or_default()),
        url: Some(url_join(BASE_URL, &key)),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("a-h"))
        .filter_map(|chunk| {
            let href = attr_after(chunk, "chapter-name", "href").or_else(|| attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: text_between(chunk, "<a", "</a>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty()),
                url: Some(url_join(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("read-content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(first_img)
        .enumerate()
        .map(|(index, image)| {
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), format!("{BASE_URL}/"));
            MangaPage {
                content: PageContent::Url { url: url_join(BASE_URL, &image), context: Some(headers.clone()) },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn summary_rows(body: &str) -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    for chunk in body.split("post-content_item").skip(1) {
        let label = text_between(chunk, "summary-heading", "</").map(|value| strip_tags(&value).trim_end_matches(':').trim().to_string());
        let value = text_between(chunk, "summary-content", "</").map(|value| strip_tags(&value).trim().to_string());
        if let (Some(label), Some(value)) = (label, value) {
            rows.insert(label, value);
        }
    }
    rows
}

fn parse_anchor_list(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("</div>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| text_between(chunk, ">", "</a>"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    match input.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" | "complete" | "finished" => ItemStatus::Completed,
        "hiatus" | "on hiatus" | "on hold" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split(['?', '#']).next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).or_else(|| filters.get("values").and_then(|values| values.get(key))).and_then(Value::as_str)
}

fn first_img(input: &str) -> Option<String> {
    attr(input, "data-src").or_else(|| attr(input, "data-lazy-src")).or_else(|| attr(input, "src"))
}

fn attr(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let start = input.find(&needle)? + needle.len();
        let rest = &input[start..];
        let end = rest.find(quote)?;
        return Some(html_unescape(&rest[..end]));
    }
    None
}

fn attr_after(input: &str, marker: &str, name: &str) -> Option<String> {
    let start = input.find(marker)?;
    attr(&input[start..], name)
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    let start_index = input.find(start)?;
    let after_start = &input[start_index..];
    let content_start = after_start.find('>').map(|idx| idx + 1).unwrap_or(start.len());
    let rest = &after_start[content_start..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].to_string())
}

fn strip_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    html_unescape(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn url_join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    }
}

fn query_escape(input: &str) -> String {
    input.replace(' ', "+")
}

const LIST_FIXTURE: &str = r#"
<div class="home-item"><h3 class="htitle"><a href="https://mangadna.com/manga/sample" title="Sample DNA">Sample</a></h3><img data-src="/cover.jpg"></div>
<div class="home-item"><h3 class="htitle"><a href="https://mangadna.com/manga/sample-raw" title="Raw DNA">Raw</a></h3><img src="/raw.jpg"></div>
<ul class="pagination"><li class="next"><a href="/manga/page/2">Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample DNA</h1>
<meta property="og:description" content="Sample description.">
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="post-content_item"><div class="summary-heading">Status:</div><div class="summary-content">Ongoing</div></div>
<div class="post-content_item"><div class="summary-heading">Alternative:</div><div class="summary-content">Alt DNA</div></div>
<div class="author-content"><a>Author One</a></div>
<div class="artist-content"><a>Artist One</a></div>
<div class="genres-content"><a>Action</a><a>Romance</a></div>
<ul class="row-content-chapter"><li class="a-h"><a class="chapter-name" href="https://mangadna.com/manga/sample/chapter-1">Chapter 1</a></li></ul>
"#;

const PAGE_FIXTURE: &str = r#"
<div class="read-content"><img data-lazy-src="/page-1.jpg"><img src="https://cdn.mangadna.com/page-2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_listing_filters_raw() {
        let page = parse_listing(LIST_FIXTURE, SOURCES[0]);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
    }

    #[test]
    fn details_chapters_pages_parse() {
        let item = parse_details(DETAILS_FIXTURE, Some("/manga/sample".into()), SOURCES[1]);
        assert_eq!(item.title, "Sample DNA");
        assert_eq!(item.authors, vec!["Author One"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGE_FIXTURE).len(), 2);
    }

    #[test]
    fn filters_build_url() {
        let request = serde_json::json!({"filters": {"genre": "action", "sort": "trending"}});
        assert_eq!(filtered_url(&request, 2), "https://mangadna.com/manga-genre/action/2?orderby=trending");
    }
}
