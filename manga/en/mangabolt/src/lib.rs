use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaBolt = MangaBolt;
const BASE_URL: &str = "https://mangabolt.com";

struct MangaBolt;

impl MangaSource for MangaBolt {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_popular(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(Paged {
                entries: parse_latest(&fetch_document_or_fixture(
                    &format!("{BASE_URL}/latest"),
                    LATEST_FIXTURE,
                )),
                has_next_page: false,
            })
        } else {
            Ok(Paged {
                entries: parse_popular(&fetch_document_or_fixture(
                    &format!("{BASE_URL}/storage/manga-list.html"),
                    LIST_FIXTURE,
                )),
                has_next_page: false,
            })
        }
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let needle = query.to_ascii_lowercase();
        let entries = parse_popular(&fetch_document_or_fixture(
            &format!("{BASE_URL}/storage/manga-list.html"),
            LIST_FIXTURE,
        ))
        .into_iter()
        .filter(|item| item.title.to_ascii_lowercase().contains(&needle))
        .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/sample-chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".com/") {
            return format!("/{}", value[index + 5..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .filter(|chunk| chunk.contains("onclick="))
        .filter_map(|chunk| {
            let onclick = html::attr(chunk, "onclick")?;
            let path = onclick.split('\'').nth(1)?;
            let key = normalize_key(path);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .or_else(|| html::text_between(chunk, "item-title", "</"))
                .map(|value| {
                    html::strip_tags(&value)
                        .replace('🔥', "")
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("bg-bg-secondary")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let slug = href
                .split("/chapter/")
                .nth(1)?
                .split("-chapter-")
                .next()?
                .trim_matches('/');
            if slug.is_empty() {
                return None;
            }
            let key = format!("/manga/{slug}");
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "font-bold", "</")
                    .map(|value| html::strip_tags(&value))
                    .map(|value| {
                        value
                            .split("Chapter")
                            .next()
                            .unwrap_or(&value)
                            .trim()
                            .to_string()
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "main-content", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "<img", "src").map(|value| absolute_url(&value)),
        description: html::text_between(body, "text-text-muted", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Ongoing,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("bg-bg-secondary")
        .skip(1)
        .filter(|chunk| chunk.contains("div.grid") || chunk.contains("/chapter/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".to_string())),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("js-page"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|value| !value.is_empty() && !value.starts_with("data:image"))
        .fold(Vec::<String>::new(), |mut images, image| {
            let absolute = absolute_url(&image);
            if !images.contains(&absolute) {
                images.push(absolute);
            }
            images
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="section-header" onclick="location.href='/manga/sample/'"><h2>Sample Manga🔥</h2></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div class="bg-bg-secondary"><a href="/chapter/sample-chapter-1"><img src="/cover.jpg"></a><div class="font-bold">Sample Manga Chapter 1</div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="main-content"><h1>Sample Manga</h1></div><div class="flex"><img src="/cover.jpg"></div>
<div class="text-text-muted">A sample.</div>
<div class="bg-bg-secondary"><div class="grid"><a href="/chapter/sample-chapter-1">Chapter 1</a></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="js-pages-container"><img class="js-page" src="/page1.jpg"><img class="js-page" data-src="/page2.jpg"></div>"#;
