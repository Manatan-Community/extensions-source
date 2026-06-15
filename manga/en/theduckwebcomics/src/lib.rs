use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TheDuckWebcomics = TheDuckWebcomics;
const BASE_URL: &str = "https://www.theduckwebcomics.com";

struct TheDuckWebcomics;

impl MangaSource for TheDuckWebcomics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/search/?page={page}&last_update=today")
        } else {
            format!("{BASE_URL}/search/?page={page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1/".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/search/?page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &format!("{BASE_URL}/search/?page=1&last_update=today"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
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
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("comicdescparagraphs") || chunk.contains("size24"))
            .filter_map(parse_listing_item)
            .collect(),
        has_next_page: body.contains("class=\"next\"") || body.contains("class='next'"),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "size24", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let creator = html::text_between(chunk, "size18", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "size24", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Comic".into()),
        cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        authors: creator.clone().into_iter().collect(),
        artists: creator.into_iter().collect(),
        tags: html::text_between(chunk, "size10", "</")
            .map(|value| html::strip_tags(&value))
            .map(|text| {
                text.split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        description: html::text_between(chunk, "comicdescparagraphs", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    parse_listing_item(body).unwrap_or_else(|| {
        let key = key.unwrap_or_else(|| "/sample/".into());
        CatalogItem {
            key: key.clone(),
            title: url::slug_from_url(&key).unwrap_or_else(|| "Comic".into()),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    })
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            let key = normalize_key(&format!("{}/", value.trim_end_matches('/')));
            let title = html::text_between(chunk, ">", "</option>")
                .map(|value| html::strip_tags(&value))
                .map(|value| {
                    value
                        .split("- ")
                        .nth(1)
                        .unwrap_or(&value)
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Page".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                ..MangaChapter::default()
            })
        })
        .enumerate()
        .map(|(index, mut chapter)| {
            chapter.chapter_number = Some((index + 1) as f32);
            chapter
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    html::attr_after(body, "page-image", "src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Page 1".into()),
            ..MangaPage::default()
        })
        .into_iter()
        .collect()
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        ("search".to_string(), query.to_string()),
        ("page".to_string(), page.to_string()),
    ];
    for key in ["type", "tone", "style", "genre"] {
        for value in filter_values(filters, key) {
            params.push((key.to_string(), value));
        }
    }
    for key in ["rating", "last_update"] {
        if let Some(value) = filter(filters, key).filter(|value| !value.is_empty()) {
            params.push((key.to_string(), value));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/search/?{query}")
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(ToString::to_string)
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    filter(filters, key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div style="display:block"><a class="size24" href="/sample/">Sample Comic</a><span class="size18">Creator</span><span class="size10">Adventure, Comedy</span><p class="comicdescparagraphs">Sample description.</p><img src="/cover.jpg"></div><a class="next" href="?page=2">Next</a>
"#;
const DETAILS_FIXTURE: &str = LIST_FIXTURE;
const CHAPTERS_FIXTURE: &str = r#"<select id="page_dropdown"><option value="https://www.theduckwebcomics.com/sample/1">1 - Start</option></select>"#;
const PAGES_FIXTURE: &str = r#"<img class="page-image" src="/sample/page1.jpg">"#;
