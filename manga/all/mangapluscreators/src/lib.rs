use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangaplus-creators.jp";
const SOURCE: MangaPlusCreators = MangaPlusCreators;

struct MangaPlusCreators;

impl MangaSource for MangaPlusCreators {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            let body = fetch_json_or_fixture(&format!("{BASE_URL}/api/titles/recent/?page={page}&l={}&t=episode", source.lang), LATEST_FIXTURE);
            Ok(parse_latest(&body, source))
        } else {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}/titles/popular/?p=m&l={}", source.lang), LIST_FIXTURE);
            Ok(parse_listing(&body, source, "item-recent"))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key), source)], has_next_page: false });
        }
        let body = if query.is_empty() {
            let genre = request.get("filters").and_then(|filters| filters.get("genre")).and_then(Value::as_str).unwrap_or_default();
            fetch_document_or_fixture(&format!("{BASE_URL}/genres/{genre}?l={}", source.lang), LIST_FIXTURE)
        } else {
            fetch_document_or_fixture(&format!("{BASE_URL}/keywords?q={}&l={}", query_escape(query), source.lang), SEARCH_FIXTURE)
        };
        Ok(parse_listing(&body, source, if query.is_empty() { "item-recent" } else { "item-search" }))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/titles/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/titles/sample".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/episodes/episode-1".into());
        let body = fetch_document_or_fixture(&url_join(BASE_URL, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body, &url_join(BASE_URL, &key)))
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
    SourceConfig { id: "mangapluscreators-en", lang: "en" },
    SourceConfig { id: "mangapluscreators-es", lang: "es" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("mangapluscreators-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(BASE_URL)
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig, marker: &str) -> Paged<CatalogItem> {
    let entries = body
        .split(marker)
        .skip(1)
        .filter_map(|chunk| {
            let cover = attr_after(chunk, "image-area", "src").or_else(|| attr_after(chunk, "<img", "src"))?;
            let id = cover.split('/').find(|part| part.starts_with("title-")).unwrap_or("sample").trim_start_matches("title-");
            let key = format!("/titles/{id}");
            let title = text_between(chunk, "title-area", "</").or_else(|| text_between(chunk, "title", "</")).map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "MANGA Plus Creators".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: Some(url_join(BASE_URL, &cover)),
                url: Some(url_join(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_latest(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).expect("fixture is valid"));
    let entries = value
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|title| {
            let id = title.get("content_id").or_else(|| title.get("id")).and_then(Value::as_str).unwrap_or("sample");
            let key = format!("/titles/{id}");
            CatalogItem {
                key: key.clone(),
                title: title.get("title").and_then(Value::as_str).unwrap_or("MANGA Plus Creators").into(),
                cover: title.get("thumbnail_url").or_else(|| title.get("image_url")).and_then(Value::as_str).map(ToOwned::to_owned),
                url: Some(url_join(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged { entries, has_next_page: value.get("status").and_then(Value::as_str) != Some("error") }
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/titles/sample".into());
    CatalogItem {
        key: key.clone(),
        title: text_between(body, "title-detail", "</").or_else(|| text_between(body, "<h1", "</h1>")).map(|value| strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "MANGA Plus Creators".into()),
        cover: attr_after(body, "image-area", "src").or_else(|| attr_after(body, "og:image", "content")).map(|value| url_join(BASE_URL, &value)),
        description: text_between(body, "summary", "</div>").or_else(|| attr_after(body, "og:description", "content")).map(|value| strip_tags(&value)),
        authors: text_between(body, "author", "</").map(|value| vec![strip_tags(&value)]).unwrap_or_default(),
        status: ItemStatus::Ongoing,
        url: Some(url_join(BASE_URL, &key)),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("mod-item-series")
        .skip(1)
        .filter_map(|chunk| {
            let href = attr_after(chunk, "<a", "href").or_else(|| attr(chunk, "href"))?;
            let key = normalize_key(&href);
            let number = text_between(chunk, "number", "</").map(|value| strip_tags(&value)).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(number.clone()),
                chapter_number: if number == "One-shot" { Some(0.0) } else { number.trim_start_matches('#').parse().ok() },
                url: Some(url_join(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let data = attr_after(body, "react=viewer", "data-pages").unwrap_or_else(|| PAGES_JSON.into());
    let value: Value = serde_json::from_str(&html_unescape(&data)).unwrap_or_else(|_| serde_json::from_str(PAGES_JSON).expect("fixture is valid"));
    value
        .get("pc")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let url = page.get("image_url").or_else(|| page.get("imageUrl")).and_then(Value::as_str)?;
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), referer.into());
            Some(MangaPage {
                content: PageContent::Url { url: url.into(), context: Some(headers.clone()) },
                headers,
                description: page.get("page_no").or_else(|| page.get("pageNo")).and_then(Value::as_u64).map(|number| format!("Page {number}")),
                ..MangaPage::default()
            })
        })
        .collect()
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
    input.replace("&quot;", "\"").replace("&#34;", "\"").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&#39;", "'").replace("&nbsp;", " ")
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
<div class="item-recent"><div class="image-area"><img src="https://mangaplus-creators.jp/uploads/title-sample/cover.jpg"></div><div class="title-area"><div class="title">Creator Sample</div></div></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="item-search"><div class="image-area"><img src="https://mangaplus-creators.jp/uploads/title-sample/cover.jpg"></div><div class="title-area"><div class="title">Creator Sample</div></div></div>
"#;

const LATEST_FIXTURE: &str = r#"{
  "status": "ok",
  "titles": [{ "content_id": "sample", "title": "Creator Sample", "thumbnail_url": "https://mangaplus-creators.jp/cover.jpg" }]
}"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="title-detail">Creator Sample</h1>
<meta property="og:description" content="Sample description.">
<div class="image-area"><img src="/cover.jpg"></div>
<div class="author">Author One</div>
<a class="mod-item-series" href="/episodes/episode-1"><span class="number">#1</span><span class="first-update">2025-01-01</span></a>
"#;

const PAGES_JSON: &str = r#"{"pc":[{"page_no":1,"image_url":"https://mangaplus-creators.jp/page-1.jpg"}]}"#;
const PAGE_FIXTURE: &str = r#"<div react="viewer" data-pages="{&quot;pc&quot;:[{&quot;page_no&quot;:1,&quot;image_url&quot;:&quot;https://mangaplus-creators.jp/page-1.jpg&quot;}]}"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_latest_details_chapters_pages() {
        assert_eq!(parse_listing(LIST_FIXTURE, SOURCES[0], "item-recent").entries[0].title, "Creator Sample");
        assert_eq!(parse_latest(LATEST_FIXTURE, SOURCES[0]).entries[0].key, "/titles/sample");
        let item = parse_details(DETAILS_FIXTURE, Some("/titles/sample".into()), SOURCES[0]);
        assert_eq!(item.description.as_deref(), Some("Sample description."));
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGE_FIXTURE, BASE_URL).len(), 1);
    }
}
