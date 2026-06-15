use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Manhwa18Cc = Manhwa18Cc;
const CONFIG: manga::MadaraConfig = manga::MadaraConfig {
    base_url: "https://manhwa18.cc",
    lang: "all",
    content_rating: "adult",
    manga_path: "webtoon",
    popular_url_marker: "<div class=\"manga-item",
    use_load_more: false,
    latest_enabled: true,
};

struct Manhwa18Cc;

impl MangaSource for Manhwa18Cc {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if source.lang == "ko" && !latest {
            format!("{}/raw/{page}", CONFIG.base_url)
        } else {
            let order = if latest { "" } else { "?orderby=trending" };
            format!("{}/webtoons/{page}{order}", CONFIG.base_url)
        };
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
            has_next_page: body.contains("pagination") && body.contains("next"),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains("/webtoon/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &body,
                    Some(CONFIG.normalize_manga_key(query)),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let target = format!(
            "{}/search?q={}&page={page}",
            CONFIG.base_url,
            encode_query(query)
        );
        let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
            has_next_page: body.contains("pagination") && body.contains("next"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(manga::Madara::parse_details(&body, Some(key), &CONFIG))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/webtoon/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) && input.contains("/webtoon/") {
            let body = manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &body,
                    Some(CONFIG.normalize_manga_key(input)),
                    &CONFIG,
                )),
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
    lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "manhwa18cc-en",
        lang: "en",
    },
    SourceConfig {
        id: "manhwa18cc-ko",
        lang: "ko",
    },
    SourceConfig {
        id: "manhwa18cc-all",
        lang: "all",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("manhwa18cc-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn parse_listing(body: &str, source: SourceConfig) -> Vec<CatalogItem> {
    body.split("manga-item")
        .skip(1)
        .filter(|chunk| match source.lang {
            "en" => !chunk.contains("Raw"),
            "ko" => chunk.contains("Raw"),
            _ => true,
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|text| html::strip_tags(&text))
                })
                .unwrap_or_else(|| "Manhwa18.cc".into());
            let key = CONFIG.normalize_manga_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|cover| CONFIG.absolute_url(&cover)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some(source.lang.into()),
                content_rating: Some(CONFIG.content_rating.into()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_chapters(body: &str, manga_key: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("a-h") || chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(CONFIG.absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                language: Some(source.lang.into()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.into(),
            title: Some("Read".into()),
            url: Some(CONFIG.absolute_url(manga_key)),
            language: Some(source.lang.into()),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let scope = body.split("read-content").nth(1).unwrap_or(body);
    scope
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: CONFIG.absolute_url(&image),
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="manga-item"><div class="data"><a href="https://manhwa18.cc/webtoon/sample" title="Sample 18">Sample 18</a></div><img data-src="https://manhwa18.cc/cover.jpg"></div>
<ul class="pagination"><li class="next"><a href="/webtoons/2">Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample 18</h1></div>
<div class="summary_image"><img src="https://manhwa18.cc/cover.jpg"></div>
<div class="summary__content">Adult sample.</div>
<li class="a-h"><a href="https://manhwa18.cc/webtoon/sample/chapter-1">Chapter 1</a><span class="chapter-time">01 Jan 2024</span></li>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="read-content"><img src="https://manhwa18.cc/page-1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manhwa18cc() {
        assert_eq!(parse_listing(LIST_FIXTURE, SOURCES[0]).len(), 1);
        assert_eq!(
            manga::Madara::parse_details(DETAILS_FIXTURE, None, &CONFIG).title,
            "Sample 18"
        );
        assert_eq!(
            parse_chapters(DETAILS_FIXTURE, "/webtoon/sample", SOURCES[0]).len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
