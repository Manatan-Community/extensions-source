use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://hennojin.com";
const SOURCE: Hennojin = Hennojin;
const WP_NONCE: &str = "40229f97a5";

struct Hennojin;

impl MangaSource for Hennojin {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = list_url(source, page);
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key), source)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/home/page/{page}?keyword={}&_wpnonce={WP_NONCE}",
            url::query_escape(query)
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample?view=multi".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), source)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
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

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("hennojin-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(source: SourceConfig, page: u64) -> String {
    if source.lang == "ja" {
        format!("{BASE_URL}/home/page/{page}/?archive=raw")
    } else {
        format!("{BASE_URL}/home/page/{page}")
    }
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("layer-content")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "title_link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "title_link", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Hennojin Gallery".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(source.lang.to_string()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("paginate") && body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let tags = detail_links(body, &["/parody/", "/tags/", "/character/"]);
    let artist = detail_links(body, &["/artist/"]).into_iter().next();
    let author = detail_links(body, &["/group/"])
        .into_iter()
        .next()
        .or_else(|| artist.clone());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Hennojin Gallery".into())),
        cover: html::attr_after(body, "manga-thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        authors: author.into_iter().collect(),
        artists: artist.into_iter().collect(),
        description: description(body),
        tags,
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("Read Online"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = with_multi_view(&normalize_key(&href));
            Some(MangaChapter {
                key: key.clone(),
                title: Some("Chapter".into()),
                chapter_number: Some(-1.0),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: "/read/sample?view=multi".into(),
            title: Some("Chapter".into()),
            chapter_number: Some(-1.0),
            url: Some(format!("{BASE_URL}/read/sample?view=multi")),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("slideshow-container") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| !src.starts_with("data:"))
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &src),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn description(body: &str) -> Option<String> {
    html::text_between(body, "manga-subtitle", "tags-list")
        .or_else(|| html::text_between(body, "manga-subtitle", "</section>"))
        .map(|value| html::strip_tags(&value.replace("<br>", "\n").replace("<br/>", "\n")))
        .map(|value| value.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n"))
        .filter(|value| !value.is_empty())
}

fn detail_links(body: &str, markers: &[&str]) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| markers.iter().any(|marker| chunk.contains(marker)))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('#')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn with_multi_view(key: &str) -> String {
    let base = key.split('?').next().unwrap_or(key);
    format!("{base}?view=multi")
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "hennojin-en", lang: "en" },
    SourceConfig { id: "hennojin-ja", lang: "ja" },
];

const LIST_FIXTURE: &str = r#"
<div class="grid-items"><div class="layer-content"><div class="title_link"><a href="https://hennojin.com/manga/sample">Sample Gallery</a></div><img src="https://hennojin.com/thumb.jpg"></div></div>
<div class="paginate"><a class="next" href="/home/page/2">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Gallery</h1>
<div class="manga-thumbnail"><img src="https://hennojin.com/cover.jpg"></div>
<div class="manga-subtitle"></div><p></p><p>Sample description<br>Second line</p>
<div class="tags-list"><a href="/artist/sample-artist">Sample Artist</a><a href="/group/sample-group">Sample Group</a><a href="/tags/outdoor">Outdoor</a></div>
<a href="https://hennojin.com/read/sample">Read Online</a>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="slideshow-container"><img src="https://hennojin.com/page-1.jpg"><img src="https://hennojin.com/page-2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hennojin() {
        let listing = parse_listing(LIST_FIXTURE, SOURCES[0]);
        assert_eq!(listing.entries.len(), 1);
        assert!(listing.has_next_page);
        let details = parse_details(DETAILS_FIXTURE, Some("/manga/sample".into()), SOURCES[0]);
        assert_eq!(details.title, "Sample Gallery");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
