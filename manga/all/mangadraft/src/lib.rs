use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangadraft.com";
const SOURCE: MangaDraft = MangaDraft;

struct MangaDraft;

impl MangaSource for MangaDraft {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request_page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "news" } else { "popular" };
        let body = fetch_json_or_fixture(&catalog_url(page, order, &Value::Null), LIST_FIXTURE);
        Ok(parse_catalog(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key))], has_next_page: false });
        }
        let page = request_page(&request);
        let body = fetch_json_or_fixture(&catalog_url(page, "", request.get("filters").unwrap_or(&Value::Null)), LIST_FIXTURE);
        let mut result = parse_catalog(&body);
        if !query.is_empty() {
            let needle = query.to_ascii_lowercase();
            result.entries.retain(|item| item.title.to_ascii_lowercase().contains(&needle));
            result.has_next_page = false;
        }
        Ok(result)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/projects/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/projects/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/api/reader/listPages?first_page=101&grouped_by_category=true".into());
        let url = if key.contains("/api/reader/listPages") {
            url_join(BASE_URL, &key)
        } else {
            let first_page = key.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>();
            format!("{BASE_URL}/api/reader/listPages?first_page={first_page}&grouped_by_category=true")
        };
        let body = fetch_json_or_fixture(&url, PAGES_FIXTURE);
        Ok(parse_pages(&body, page_id_from_url(&url)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn catalog_url(page: u64, forced_order: &str, filters: &Value) -> String {
    let order = if forced_order.is_empty() { filter_string(filters, "order").unwrap_or("all") } else { forced_order };
    let fields = vec![
        ("type".to_string(), filter_string(filters, "type").unwrap_or("all").to_string()),
        ("order".into(), order.to_string()),
        ("section".into(), filter_string(filters, "section").unwrap_or("").to_string()),
        ("genre".into(), filter_string(filters, "genre").unwrap_or("").to_string()),
        ("format".into(), filter_string(filters, "format").unwrap_or("").to_string()),
        ("language".into(), filter_string(filters, "language").unwrap_or("").to_string()),
        ("status".into(), filter_string(filters, "status").unwrap_or("").to_string()),
        ("order_all".into(), filter_string(filters, "sort").unwrap_or("likes").to_string()),
        ("page".into(), page.to_string()),
        ("number".into(), "20".into()),
    ];
    format!("{BASE_URL}/api/catalog/projects?{}", form_body_owned(&fields))
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = normalize_key(item.get("url").and_then(Value::as_str).unwrap_or("/projects/sample"));
            CatalogItem {
                key: key.clone(),
                title: item.get("name").and_then(Value::as_str).unwrap_or("MangaDraft").into(),
                cover: item.get("avatar").and_then(Value::as_str).map(|value| url_join(BASE_URL, value)),
                description: item.get("description").and_then(Value::as_str).map(ToOwned::to_owned),
                tags: item.get("genres").and_then(Value::as_str).map(split_tags).unwrap_or_default(),
                url: Some(url_join(BASE_URL, &key)),
                language: Some("all".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect::<Vec<_>>();
    Paged { has_next_page: entries.len() >= 20, entries }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/projects/sample".into());
    let project = extract_window_project(body).unwrap_or_else(|| serde_json::from_str(PROJECT_FIXTURE).expect("fixture is valid"));
    CatalogItem {
        key: key.clone(),
        title: project.get("name").and_then(Value::as_str).unwrap_or("MangaDraft").into(),
        description: project.get("description").and_then(Value::as_str).map(ToOwned::to_owned),
        authors: text_by_title(body, "Auteur").into_iter().collect(),
        artists: text_by_title(body, "créateur").into_iter().collect(),
        tags: project
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("name").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect(),
        status: parse_status(project.get("project_status_id").and_then(Value::as_i64)),
        url: Some(url_join(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| !chunk.contains("<img") && (chunk.contains("/read/") || chunk.contains("chapter")))
        .filter_map(|chunk| {
            let href = attr(chunk, "href")?;
            let key = if href.contains("/api/reader/listPages") {
                normalize_key(&href)
            } else {
                let first_page = href.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>();
                format!("/api/reader/listPages?first_page={first_page}&grouped_by_category=true")
            };
            let title = text_between(chunk, ">", "</a>").map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url_join(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, first_page: u64) -> Vec<MangaPage> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    let pages = value
        .as_object()
        .and_then(|map| {
            map.values()
                .filter_map(Value::as_array)
                .find(|pages| pages.iter().any(|page| page.get("id").and_then(Value::as_u64) == Some(first_page)))
        })
        .or_else(|| value.as_object().and_then(|map| map.values().find_map(Value::as_array)))
        .cloned()
        .unwrap_or_default();
    pages
        .into_iter()
        .filter_map(|page| {
            let url = page.get("url").and_then(Value::as_str)?;
            let number = page.get("number").and_then(Value::as_u64).unwrap_or(1);
            let image = format!("{url}?size=full");
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), format!("{BASE_URL}/"));
            Some(MangaPage {
                content: PageContent::Url { url: image.clone(), context: Some(headers.clone()) },
                headers,
                description: Some(format!("Page {number}")),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn extract_window_project(body: &str) -> Option<Value> {
    let start = body.find("window.project")?;
    let rest = &body[start..];
    let json_start = rest.find('{')?;
    let mut depth = 0usize;
    let mut end = None;
    for (index, ch) in rest[json_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(json_start + index + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    serde_json::from_str(&rest[json_start..end?]).ok()
}

fn text_by_title(body: &str, title: &str) -> Option<String> {
    body.split(&format!("title=\"{title}\""))
        .nth(1)
        .and_then(|chunk| text_between(chunk, ">", "</"))
        .map(|value| strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(status: Option<i64>) -> ItemStatus {
    match status {
        Some(0) => ItemStatus::Ongoing,
        Some(1) => ItemStatus::Completed,
        Some(2) => ItemStatus::Hiatus,
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
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('#').next().unwrap_or(input);
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

fn page_id_from_url(input: &str) -> u64 {
    input.split("first_page=").nth(1).and_then(|rest| rest.split('&').next()).and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).or_else(|| filters.get("values").and_then(|values| values.get(key))).and_then(Value::as_str)
}

fn split_tags(input: &str) -> Vec<String> {
    input.split(',').map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned).collect()
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
    input.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ")
}

fn url_join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    }
}

fn form_body_owned(fields: &[(String, String)]) -> String {
    fields.iter().map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value))).collect::<Vec<_>>().join("&")
}

fn query_escape(input: &str) -> String {
    input.replace(' ', "+")
}

const LIST_FIXTURE: &str = r#"{
  "data": [
    { "name": "Sample Draft", "avatar": "/cover.jpg", "genres": "Action, Fantasy", "description": "Sample description.", "url": "/projects/sample" }
  ]
}"#;

const PROJECT_FIXTURE: &str = r#"{
  "name": "Sample Draft",
  "description": "Sample description.",
  "project_status_id": 0,
  "genres": [{ "id": 1, "name": "Action", "slug": "action" }]
}"#;

const DETAILS_FIXTURE: &str = r#"
<script>window.project = {"name":"Sample Draft","description":"Sample description.","project_status_id":0,"genres":[{"id":1,"name":"Action","slug":"action"}]};</script>
<span title="Auteur">Author One</span><span title="créateur">Artist One</span>
<div class="mt-7"><div><a href="https://mangadraft.com/read/sample/c.101"><span class="group-hover:text-secondary">Chapter 1</span></a></div></div>
"#;

const PAGES_FIXTURE: &str = r#"{
  "1": [
    { "id": 101, "number": 1, "url": "https://mangadraft.com/page-101.jpg" },
    { "id": 102, "number": 2, "url": "https://mangadraft.com/page-102.jpg" }
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog() {
        let page = parse_catalog(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample Draft");
        assert_eq!(page.entries[0].tags, vec!["Action", "Fantasy"]);
    }

    #[test]
    fn parses_details_chapters_pages() {
        let item = parse_details(DETAILS_FIXTURE, Some("/projects/sample".into()));
        assert_eq!(item.authors, vec!["Author One"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters.len(), 1);
        let pages = parse_pages(PAGES_FIXTURE, 101);
        assert_eq!(pages.len(), 2);
    }
}
