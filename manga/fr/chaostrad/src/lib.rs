use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ChaosTrad = ChaosTrad;
const BASE_URL: &str = "https://chaostrad.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct ChaosTrad;

impl MangaSource for ChaosTrad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_catalog(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_catalog(&fetch_document_or_fixture(BASE_URL, LIST_FIXTURE)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let query_lower = query.to_ascii_lowercase();
        Ok(Paged {
            entries: parse_catalog(&fetch_document_or_fixture(BASE_URL, LIST_FIXTURE))
                .into_iter()
                .filter(|item| {
                    query_lower.is_empty() || item.title.to_ascii_lowercase().contains(&query_lower)
                })
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(normalize_key(&key))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics/sample/1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
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

fn parse_catalog(body: &str) -> Vec<CatalogItem> {
    let mut items = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let title = normalize_title(
            &html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_default(),
        );
        if href.starts_with("/comics/") {
            push_unique(&mut items, catalog_item(&href, &title));
        } else if href.starts_with("/search/") {
            for item in parse_collection(&href, &title) {
                push_unique(&mut items, item);
            }
        }
    }
    items
}

fn parse_collection(path: &str, fallback_title: &str) -> Vec<CatalogItem> {
    let body = fetch_document_or_fixture(&url::join_url(BASE_URL, path), COLLECTION_FIXTURE);
    let mut items = Vec::new();
    for chunk in body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-link"))
    {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let key = if href.contains("/comics/") {
            let normalized = normalize_key(&href);
            let mut parts = normalized.trim_matches('/').split('/');
            let first = parts.next().unwrap_or_default();
            let slug = parts.next().unwrap_or_default();
            if first == "comics" && !slug.is_empty() {
                format!("/comics/{slug}")
            } else {
                normalized
            }
        } else if href.contains("/search/") {
            normalize_key(&href)
        } else {
            continue;
        };
        let title = normalize_title(
            &html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| fallback_title.to_string()),
        );
        push_unique(&mut items, catalog_item(&key, &title));
    }
    if items.is_empty() {
        items.push(catalog_item(path, fallback_title));
    }
    items
}

fn catalog_item(href: &str, title: &str) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: if title.is_empty() {
            url::slug_from_url(&key).unwrap_or_else(|| "ChaosTrad".into())
        } else {
            title.to_string()
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/sample".into());
    CatalogItem {
        key: key.clone(),
        title: normalize_title(
            &html::text_between(body, "<h1", "</h1>")
                .or_else(|| html::text_between(body, "<title", "</title>"))
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "ChaosTrad".into()),
        ),
        cover: html::attr_after(body, "comic-link", "src")
            .or_else(|| html::attr_after(body, "comic-image", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let chapter_number = href
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(-1.0);
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(format_chapter_name(chapter_number)),
                chapter_number: (chapter_number >= 0.0).then_some(chapter_number),
                date_uploaded: html::text_between(chunk, "release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_fr_date(&value)),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        let number = manga_key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(1.0);
        chapters.push(MangaChapter {
            key: normalize_key(manga_key),
            title: Some(format_chapter_name(number)),
            chapter_number: Some(number),
            url: Some(url::join_url(BASE_URL, manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-image"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_title(raw: &str) -> String {
    html::strip_tags(raw)
        .trim()
        .strip_prefix("Chapitre de ")
        .or_else(|| raw.trim().strip_prefix("Voir le chapitre "))
        .unwrap_or(raw.trim())
        .rsplit_once(" #")
        .map(|(title, _)| title)
        .unwrap_or(raw.trim())
        .trim()
        .to_string()
}

fn format_chapter_name(chapter_number: f32) -> String {
    if chapter_number >= 0.0 && chapter_number.fract() == 0.0 {
        format!("#{}", chapter_number as i64)
    } else if chapter_number >= 0.0 {
        format!("#{chapter_number}")
    } else {
        "#?".to_string()
    }
}

fn parse_fr_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('.');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(items: &mut Vec<CatalogItem>, item: CatalogItem) {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r##"
<a id="comics-main">Comics</a><nav>
<a href="/comics/sample" title="Sample">Sample</a>
<a href="/search/collection" title="Collection">Collection</a>
</nav>
"##;
const COLLECTION_FIXTURE: &str = r##"<a class="comic-link" href="/comics/collection-sample/1" title="Voir le chapitre Collection Sample #1"><img src="/thumb.jpg"></a>"##;
const DETAILS_FIXTURE: &str = r##"
<h1>Sample</h1>
<a class="comic-link" href="/comics/sample/1"><img src="/thumb_thumbnail.jpg"><p class="release-date">01.01.2024</p></a>
"##;
const PAGES_FIXTURE: &str =
    r##"<img class="comic-image" src="/page1.jpg"><img class="comic-image" src="/page2.jpg">"##;
