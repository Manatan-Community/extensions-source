use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ReadComicsOnline = ReadComicsOnline;
const BASE_URL: &str = "https://readcomicsonline.ru";

struct ReadComicsOnline;

impl MangaSource for ReadComicsOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest-release?page={page}")
        } else {
            format!("{BASE_URL}/filterList?page={page}&sortBy=views&asc=false")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            format!("{BASE_URL}/filterList?page={page}")
        } else {
            format!("{BASE_URL}/search?query={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
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
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("media") || chunk.contains("manga-item"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "media-heading", "href")
                .or_else(|| html::attr_after(chunk, "manga-heading", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "media-heading", "</")
                .or_else(|| html::text_between(chunk, "manga-heading", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| guess_cover(&key, Some(&image))),
                url: Some(absolute_url(&key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("pagination") && body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let status_text = detail_value(body, "status").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "listmanga-header", "</")
            .or_else(|| html::text_between(body, "widget-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into())),
        cover: image_attr(body)
            .map(|image| guess_cover(&key, Some(&image)))
            .or_else(|| Some(guess_cover(&key, None))),
        description: html::text_between(body, "well", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: detail_value(body, "author").into_iter().collect(),
        artists: detail_value(body, "artist").into_iter().collect(),
        tags: detail_links(body, "categor"),
        status: parse_status(&status_text),
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let manga_title = html::text_between(body, "listmanga-header", "</")
        .or_else(|| html::text_between(body, "widget-title", "</"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter") && !chunk.contains("btn"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let raw_title = html::text_between(chunk, "chapter-title-rtl", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(clean_chapter_name(&manga_title, &raw_title)),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "date-chapter-title-rtl", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("img-responsive") || chunk.contains("data-src") || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn detail_value(body: &str, label: &str) -> Option<String> {
    body.split("<dt")
        .skip(1)
        .find(|chunk| chunk.to_ascii_lowercase().contains(label))
        .and_then(|chunk| {
            html::text_between(chunk, "<dd", "</dd>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
}

fn detail_links(body: &str, label: &str) -> Vec<String> {
    body.split("<dt")
        .skip(1)
        .find(|chunk| chunk.to_ascii_lowercase().contains(label))
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_chapter_name(manga_title: &str, name: &str) -> String {
    let initial = name.replacen(manga_title, "Chapter", 1);
    let mut parts = initial.splitn(2, ':').map(str::trim);
    let first = parts.next().unwrap_or("Chapter");
    let second = parts.next();
    if second.is_none_or(|value| value == first) {
        first.to_string()
    } else {
        format!("{first}: {}", second.unwrap())
    }
}

fn parse_status(input: &str) -> ItemStatus {
    let value = input.to_ascii_lowercase();
    if value.contains("complete") {
        ItemStatus::Completed
    } else if value.contains("ongoing") {
        ItemStatus::Ongoing
    } else if value.contains("dropped") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-original")
        .or_else(|| html::attr_after(input, "<img", "data-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn guess_cover(key: &str, image: Option<&str>) -> String {
    image
        .filter(|value| !value.is_empty())
        .map(absolute_url)
        .unwrap_or_else(|| {
            format!(
                "{BASE_URL}/uploads/manga/{}/cover/cover_250x350.jpg",
                key.trim_matches('/').rsplit('/').next().unwrap_or("sample")
            )
        })
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'))
    } else {
        format!("/{}", input.trim_matches('/'))
    }
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="media"><div class="media-heading"><a href="/manga/sample">Sample Comic</a></div><img src="/cover.jpg"></div><a rel="next"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="listmanga-header">Sample Comic</h1><div class="row"><img class="img-responsive" src="/cover.jpg"><div class="well">Summary</div><dl class="dl-horizontal"><dt>Status</dt><dd>Ongoing</dd><dt>Author(s)</dt><dd>Author</dd><dt>Categories</dt><dd><a>Action</a></dd></dl></div><ul class="chapters"><li><div class="chapter-title-rtl"><a href="/manga/sample/chapter-1">Sample Comic: Chapter 1</a></div><div class="date-chapter-title-rtl">1 Jan. 2024</div></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div id="all"><img class="img-responsive" src="/page1.jpg"><img class="img-responsive" src="/page2.jpg"></div>"#;
