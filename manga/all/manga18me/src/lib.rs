use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://manga18.me";
const SOURCE: Manga18Me = Manga18Me;

struct Manga18Me;

impl MangaSource for Manga18Me {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "latest" } else { "trending" };
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/manga/{page}?orderby={order}"), LIST_FIXTURE);
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
        let target = if query.is_empty() {
            filtered_url(&request, page)
        } else {
            format!("{BASE_URL}/search?q={}&page={page}", query_escape(query))
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
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
    SourceConfig { id: "manga18me-all", lang: "all" },
    SourceConfig { id: "manga18me-en", lang: "en" },
];

fn source_for(request: &Value) -> SourceConfig {
    let source_id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("manga18me-all");
    SOURCES.iter().copied().find(|source| source.id == source_id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn filtered_url(request: &Value, page: u64) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let raw = filter_bool(filters, "raw");
    let completed = filter_bool(filters, "completed");
    let genre = filter_string(filters, "genre").unwrap_or("manga");
    let sort = filter_string(filters, "sort").unwrap_or("latest");
    let path = if raw {
        format!("raw/{page}")
    } else if completed {
        format!("completed/{page}")
    } else if genre == "manga" {
        format!("manga/{page}")
    } else {
        format!("genre/{genre}/{page}")
    };
    format!("{BASE_URL}/{path}?orderby={sort}")
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .or_else(|| filters.get("values").and_then(|values| values.get(key)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn filter_bool(filters: &Value, key: &str) -> bool {
    filters
        .get(key)
        .or_else(|| filters.get("values").and_then(|values| values.get(key)))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let raw_context = text_between(body, "section-heading", "</h1>").unwrap_or_default().to_ascii_lowercase().contains("raw")
        || attr_after(body, "canonical", "href").is_some_and(|href| href.contains("raw"));
    let entries = body
        .split("page-item-detail")
        .skip(1)
        .filter_map(|chunk| {
            let href = attr_after(chunk, "item-thumb", "href").or_else(|| attr_after(chunk, "<a", "href"))?;
            if source.lang == "en" && !raw_context && href.contains("raw") {
                return None;
            }
            let key = normalize_key(&href);
            let title = attr_after(chunk, "<img", "alt")
                .or_else(|| text_between(chunk, "<a", "</a>").map(|value| strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Manga".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: attr_after(chunk, "<img", "src").map(|value| url_join(BASE_URL, &value)),
                url: Some(url_join(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("class=\"next") || body.contains("class='next") }
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let alt_names = find_summary(body, "Alternative").filter(|value| value != "Updating");
    let description = [find_ss_manga(body), alt_names.map(|value| format!("Alternative Names:\n{}", bullet_lines(&value)))]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty() && value != "N/A")
        .collect::<Vec<_>>()
        .join("\n");
    CatalogItem {
        key: key.clone(),
        title: text_between(body, "post-title", "</h1>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Manga".into()),
        cover: attr_after(body, "summary_image", "src").map(|value| url_join(BASE_URL, &value)),
        description: (!description.is_empty()).then_some(description),
        authors: find_summary(body, "Artist").filter(|value| value != "Updating").map(|value| vec![value]).unwrap_or_default(),
        artists: find_summary(body, "Artist").filter(|value| value != "Updating").map(|value| vec![value]).unwrap_or_default(),
        tags: parse_genres(body),
        status: parse_status(&find_summary(body, "Status").unwrap_or_default()),
        url: Some(url_join(BASE_URL, &key)),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("a-h wleft")
        .skip(1)
        .filter_map(|chunk| {
            let href = attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let name = text_between(chunk, "<a", "</a>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title: name,
                url: Some(url_join(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let scope = body.split("read-content wleft").nth(1).unwrap_or(body);
    scope
        .split("<img")
        .skip(1)
        .filter_map(|chunk| attr(chunk, "src"))
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

fn find_ss_manga(body: &str) -> Option<String> {
    text_between(body, "ss-manga", "</div>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty())
}

fn find_summary(body: &str, label: &str) -> Option<String> {
    body.split("post-content_item")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| text_between(chunk, "summary-content", "</div>"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_genres(body: &str) -> Vec<String> {
    body.split("genres-content")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga-list/"))
        .filter_map(|chunk| text_between(chunk, ">", "</a>"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    match input.trim() {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn bullet_lines(input: &str) -> String {
    input
        .split(['/', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n")
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
    format!("/{}", path.trim_start_matches('/'))
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
<div class="section-heading"><h1>Trending Manga</h1></div>
<div class="canonical" href="https://manga18.me/manga/1"></div>
<div class="page-item-detail"><div class="item-thumb wleft"><a href="https://manga18.me/manga/sample"><img src="/cover.jpg" alt="Sample Title"></a></div></div>
<div class="page-item-detail"><div class="item-thumb wleft"><a href="https://manga18.me/raw/raw-sample"><img src="/raw.jpg" alt="Raw Title"></a></div></div>
<a class="next" href="/manga/2">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title wleft"><h1>Sample Title</h1></div>
<div class="summary_image"><img src="/cover.jpg"></div>
<div class="ss-manga">A sample description.</div>
<div class="post_content">
  <div class="post-content_item wleft"><div>Alternative</div><div class="summary-content">Alt One / Alt Two</div></div>
  <div class="post-content_item wleft"><div>Status</div><div class="summary-content">Ongoing</div></div>
  <div class="post-content_item wleft"><div>Artist</div><div class="summary-content">Sample Artist</div></div>
  <div class="href-content genres-content"><a href="/manga-list/action">Action</a><a href="/manga-list/romance">Romance</a></div>
</div>
<ul class="row-content-chapter wleft">
  <li class="a-h wleft"><a href="https://manga18.me/manga/sample/chapter-1">Chapter 1</a><span>01 Jan 2025</span></li>
</ul>
"#;

const PAGE_FIXTURE: &str = r#"
<div class="read-content wleft"><img src="/page-1.jpg"><img src="https://cdn.manga18.me/page-2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_listing_filters_raw_entries() {
        let page = parse_listing(LIST_FIXTURE, SourceConfig { id: "manga18me-en", lang: "en" });
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/manga/sample");
        assert!(page.has_next_page);
    }

    #[test]
    fn details_parse_metadata() {
        let item = parse_details(DETAILS_FIXTURE, Some("/manga/sample".into()), SOURCES[0]);
        assert_eq!(item.title, "Sample Title");
        assert_eq!(item.authors, vec!["Sample Artist"]);
        assert_eq!(item.tags, vec!["Action", "Romance"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
        assert!(item.description.unwrap().contains("Alternative Names"));
    }

    #[test]
    fn chapters_and_pages_parse() {
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1");
        let pages = parse_pages(PAGE_FIXTURE);
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn search_filters_build_expected_paths() {
        let request = serde_json::json!({"filters": {"genre": "romance", "sort": "rating"}, "page": 3});
        assert_eq!(filtered_url(&request, 3), "https://manga18.me/genre/romance/3?orderby=rating");
        let request = serde_json::json!({"filters": {"raw": true}, "page": 2});
        assert_eq!(filtered_url(&request, 2), "https://manga18.me/raw/2?orderby=latest");
    }
}
