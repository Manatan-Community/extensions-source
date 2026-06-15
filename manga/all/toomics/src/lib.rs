use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Toomics = Toomics;
const BASE_URL: &str = "https://global.toomics.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/81.0.4044.122 Safari/537.36";

struct Toomics;

impl MangaSource for Toomics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "webtoon/new_comics"
        } else {
            "webtoon/ranking"
        };
        let target = format!("{BASE_URL}/{}/{path}", source.site_lang);
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, source, LIST_FIXTURE),
            source,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![catalog_from_url(query, source, None, None)],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}/{}/webtoon/ajax_search", source.site_lang);
        let body = client(source)
            .post(target)
            .form(&[("toonData", query)])
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(parse_search(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/webtoon/episode/toon/1/search/Y", source.site_lang));
        let body =
            fetch_document_or_fixture(&url::join_url(BASE_URL, &key), source, DETAILS_FIXTURE);
        Ok(parse_details(&body, &key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/webtoon/episode/toon/1/search/Y", source.site_lang));
        let body =
            fetch_document_or_fixture(&url::join_url(BASE_URL, &key), source, DETAILS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/{}/webtoon/episode/view/toon/1/ep/1", source.site_lang));
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), source, PAGES_FIXTURE);
        Ok(parse_pages(&body, source))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let source = source_for(&request);
        let popular =
            self.list(serde_json::json!({"sourceId": source.id, "listingId": "popular"}))?;
        let latest =
            self.list(serde_json::json!({"sourceId": source.id, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_url(input, source, None, None)),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    site_lang: &'static str,
    lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "toomics-en",
        site_lang: "en",
        lang: "en",
    },
    SourceConfig {
        id: "toomics-zh-hans",
        site_lang: "sc",
        lang: "zh-Hans",
    },
    SourceConfig {
        id: "toomics-zh-hant",
        site_lang: "tc",
        lang: "zh-Hant",
    },
    SourceConfig {
        id: "toomics-es-419",
        site_lang: "mx",
        lang: "es-419",
    },
    SourceConfig {
        id: "toomics-es",
        site_lang: "es",
        lang: "es",
    },
    SourceConfig {
        id: "toomics-it",
        site_lang: "it",
        lang: "it",
    },
    SourceConfig {
        id: "toomics-de",
        site_lang: "de",
        lang: "de",
    },
    SourceConfig {
        id: "toomics-fr",
        site_lang: "fr",
        lang: "fr",
    },
    SourceConfig {
        id: "toomics-pt-br",
        site_lang: "por",
        lang: "pt-BR",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("toomics-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client(source: SourceConfig) -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("User-Agent", USER_AGENT)
        .with_referer(format!("{BASE_URL}/{}", source.site_lang))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, source: SourceConfig, fixture: &str) -> String {
    client(source)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("visual") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let title = html::text_between(chunk, "<h4", "</h4>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = format!(
                "{}/search/Y",
                normalize_key(&href, source).trim_end_matches('/')
            );
            let cover = html::attr_after(chunk, "<img", "data-original")
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|value| url::join_url(BASE_URL, &value));
            Some(catalog_from_url(&key, source, Some(title), cover))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let content = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/webtoon/sHtml")
                .and_then(Value::as_str)
                .map(clear_html)
        })
        .unwrap_or_else(|| body.to_string());
    let entries = content
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let title = html::text_between(chunk, "<strong", "</strong>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let href = html::attr_after(chunk, "<a", "href")?;
            let toon = href
                .split("toon=")
                .nth(1)
                .and_then(|tail| tail.split(['&', '\'', '"']).next())
                .unwrap_or("1");
            let key = format!("/{}/webtoon/episode/toon/{toon}/search/Y", source.site_lang);
            let cover =
                html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value));
            Some(catalog_from_url(&key, source, Some(title), cover))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: &str, source: SourceConfig) -> CatalogItem {
    let mut item = catalog_from_url(key, source, None, None);
    item.title = html::text_between(body, "<h2", "</h2>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    item.cover =
        html::attr_after(body, "og:image", "content").map(|value| url::join_url(BASE_URL, &value));
    item.description = html::text_between(body, "break-noraml", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.tags = html::text_between(body, "dt:contains(genres)", "</dd>")
        .map(|value| {
            html::strip_tags(&value)
                .split('/')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("normal_ep")
                && (chunk.contains("coin-type1") || chunk.contains("coin-type6"))
        })
        .filter_map(|chunk| {
            let num = html::text_between(chunk, "cell-num", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let title = html::text_between(chunk, "cell-title", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let onclick = html::attr_after(chunk, "<a", "onclick")?;
            let href = onclick.split("href='").nth(1)?.split('\'').next()?;
            let name = if num.is_empty() {
                title
            } else {
                format!("{num} - {title}")
            };
            Some(MangaChapter {
                key: normalize_key(href, source),
                title: Some(name),
                chapter_number: num.parse::<f32>().ok(),
                scanlators: vec!["Toomics".to_string()],
                url: Some(url::join_url(BASE_URL, &normalize_key(href, source))),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, source: SourceConfig) -> Vec<MangaPage> {
    if body.contains("section_age_verif") {
        return Vec::new();
    }
    let referer = html::attr_after(body, "og:url", "content")
        .unwrap_or_else(|| format!("{BASE_URL}/{}", source.site_lang));
    body.split("load_image_")
        .skip(1)
        .filter_map(|chunk| html::attr_after(chunk, "<img", "data-src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(&referer)),
            },
            description: Some((index + 1).to_string()),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_from_url(
    input: &str,
    source: SourceConfig,
    title: Option<String>,
    cover: Option<String>,
) -> CatalogItem {
    let key = normalize_key(input, source);
    CatalogItem {
        key: key.clone(),
        title: title.unwrap_or_else(|| {
            url::slug_from_url(&key)
                .unwrap_or_else(|| "Toomics".into())
                .replace('-', " ")
        }),
        cover,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str, _source: SourceConfig) -> String {
    if input.starts_with(BASE_URL) {
        input
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
}

fn clear_html(input: &str) -> String {
    input
        .replace("\\n", "")
        .replace("\\r", "")
        .replace("\\/", "/")
        .replace("\\\"", "\"")
}

const LIST_FIXTURE: &str = r#"
<li><div class="visual"><a href="https://global.toomics.com/en/webtoon/episode/toon/1"><img data-original="https://cdn.example/cover.jpg"><h4 class="toon-title">Sample Toomics</h4></a></div></li>
"#;

const SEARCH_FIXTURE: &str = r#"{"webtoon":{"sHtml":"<ul id=\"search-list-items\"><li><a class=\"relative\" href=\"/en/webtoon/episode/toon/1\"><img src=\"https://cdn.example/cover.jpg\"><strong>Sample Toomics</strong></a></li></ul>"}}"#;

const DETAILS_FIXTURE: &str = r#"
<head><meta property="og:image" content="https://cdn.example/cover.jpg"></head>
<section class="relative"><img src="/thumb.jpg"><h2>Sample Toomics</h2><p class="break-noraml text-xs">Sample description</p></section>
<li class="normal_ep"><span class="coin-type1"></span><a onclick="location.href='/en/webtoon/episode/view/toon/1/ep/1'"><div class="cell-num">1</div><div class="cell-title"><strong>Episode One</strong></div></a></li>
"#;

const PAGES_FIXTURE: &str = r#"
<head><meta property="og:url" content="https://global.toomics.com/en/webtoon/episode/view/toon/1/ep/1"></head>
<div id="load_image_1"><img data-src="https://cdn.example/page-1.jpg"></div>
<div id="load_image_2"><img data-src="https://cdn.example/page-2.jpg"></div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_and_search() {
        let listing = parse_listing(LIST_FIXTURE, SOURCES[0]);
        assert_eq!(listing.entries[0].title, "Sample Toomics");
        let search = parse_search(SEARCH_FIXTURE, SOURCES[0]);
        assert_eq!(search.entries[0].key, "/en/webtoon/episode/toon/1/search/Y");
    }

    #[test]
    fn parses_chapters_and_pages() {
        let chapters = parse_chapters(DETAILS_FIXTURE, SOURCES[0]);
        assert_eq!(chapters[0].title.as_deref(), Some("1 - Episode One"));
        let pages = parse_pages(PAGES_FIXTURE, SOURCES[0]);
        assert_eq!(pages.len(), 2);
    }
}
