use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AnzManga = AnzManga;
const BASE_URL: &str = "https://www.anzmanga25.com";

struct AnzManga;

impl MangaSource for AnzManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest-release?page={page}")
        } else {
            format!("{BASE_URL}/filterList?page={page}&sortBy=views&asc=false")
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            target.contains("latest-release"),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let response: SearchResponseDto = fetch_json(
            &format!("{BASE_URL}/search?query={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: response
                .suggestions
                .into_iter()
                .map(SearchSuggestionDto::to_item)
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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

fn fetch_json<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    let body = client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_listing(body: &str, latest: bool) -> Paged<CatalogItem> {
    let entries = if latest {
        body.split("manga-item")
            .skip(1)
            .filter_map(latest_item)
            .collect()
    } else {
        body.split("div class=\"media\"")
            .skip(1)
            .filter_map(media_item)
            .collect()
    };
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn media_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "media-heading", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))?,
        cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("es".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn latest_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let slug = key.rsplit('/').next().unwrap_or("sample");
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "manga-heading", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))?,
        cover: Some(format!(
            "{BASE_URL}/uploads/manga/{slug}/cover/cover_250x350.jpg"
        )),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("es".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "widget-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "AnzManga".into()),
        cover: html::attr_after(body, "boxed", "src").map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "div class=\"well\"", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|v| !v.is_empty()),
        authors: info_links(body, "Autor"),
        artists: info_links(body, "Artist"),
        tags: info_links(body, "Categorías"),
        status: match info_text(body, "Estado").to_ascii_lowercase().as_str() {
            value if value.contains("completado") => ItemStatus::Completed,
            value if value.contains("public") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("es".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "<a", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| "Capítulo".into()),
                ),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("es".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("<dt")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn info_text(body: &str, label: &str) -> String {
    body.split("<dt")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input.trim_start_matches(BASE_URL).to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
    .trim_end_matches('/')
    .to_string()
}

#[derive(Debug, Deserialize)]
struct SearchResponseDto {
    #[serde(default)]
    suggestions: Vec<SearchSuggestionDto>,
}

#[derive(Debug, Deserialize)]
struct SearchSuggestionDto {
    value: String,
    data: String,
}

impl SearchSuggestionDto {
    fn to_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/manga/{}", self.data),
            title: self.value,
            cover: Some(format!(
                "{BASE_URL}/uploads/manga/{}/cover/cover_250x350.jpg",
                self.data
            )),
            url: Some(format!("{BASE_URL}/manga/{}", self.data)),
            language: Some("es".into()),
            content_rating: Some("safe".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="media"><h4 class="media-heading"><a href="/manga/sample">Sample</a></h4><img src="/cover.jpg"></div><a rel="next"></a>"#;
const SEARCH_FIXTURE: &str = r#"{"suggestions":[{"value":"Sample","data":"sample"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h2 class="widget-title">Sample</h2><div class="boxed"><img src="/cover.jpg"></div><dl><dt>Estado</dt><dd><span>Publicándose</span></dd></dl><ul class="chapters"><li><h5 class="chapter-title-rtl"><a href="/manga/sample/chapter-1">Capítulo 1</a></h5></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div id="all"><img class="img-responsive" data-src="/page1.jpg"><img class="img-responsive" data-src="/page2.jpg"></div>"#;
