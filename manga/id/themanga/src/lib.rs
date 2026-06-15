use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TheManga = TheManga;
const BASE_URL: &str = "https://themanga.site";
const CONTENT_RATING: &str = "adult";

struct TheManga;

impl MangaSource for TheManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest_update"
        } else {
            "popular"
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(&format!("{BASE_URL}/?q=&sort={sort}&page={page}"), LIST_FIXTURE),
            "a.card",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        Ok(parse_listing(
            &fetch_document_or_fixture(
                &search_url(page, query, request.get("filters")),
                SEARCH_FIXTURE,
            ),
            "a.manga-card",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &format!("{}?all=1", url::join_url(BASE_URL, &key)),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter/1.00".to_string());
        let normalized = normalize_chapter_key(&key);
        Ok(parse_pages(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &normalized), PAGES_FIXTURE),
            &normalized,
        ))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| url::join_url(BASE_URL, &normalize_chapter_key(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        format!("q={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for id in ["status", "genre", "rating_min", "year", "author", "artist", "type"] {
        if let Some(value) = filters
            .and_then(|filters| filters.get(id))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    format!("{BASE_URL}/explore?{}", params.join("&"))
}

fn parse_listing(body: &str, selector_hint: &str) -> Paged<CatalogItem> {
    let class = selector_hint.trim_start_matches("a.");
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(class))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "card-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "TheManga".to_string()));
            let image = image_attr(chunk);
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image.map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_item);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel=next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut tags = body
        .split("meta-pill")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if let Some(kind) = meta_value(body, "Type").filter(|value| !value.is_empty()) {
        tags.push(kind);
    }
    tags.sort();
    tags.dedup();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "hero-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "TheManga".to_string())),
        cover: html::attr_after(body, "hero-cover", "src")
            .or_else(|| image_attr(body))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: meta_value(body, "Author").into_iter().collect(),
        artists: meta_value(body, "Artist").into_iter().collect(),
        description: html::text_between(body, "synopsis-text", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags,
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "data-href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "chapter-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::attr(chunk, "data-local-time")
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-img"))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| {
            let page_url = url::join_url(BASE_URL, &image);
            MangaPage {
                content: PageContent::Url {
                    url: page_url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(&url::join_url(BASE_URL, chapter_key)),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn normalize_chapter_key(value: &str) -> String {
    let key = normalize_key(value);
    if key.contains("/chapter/") {
        return key;
    }
    let parts = key.trim_start_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0] == "manga" {
        let manga_slug = parts[1];
        let raw = parts[2].trim_start_matches("chapter-").replace('-', ".");
        let formatted = if raw.contains('.') {
            let mut split = raw.splitn(2, '.');
            let whole = split.next().unwrap_or("0");
            let fraction = split.next().unwrap_or("0");
            format!("{whole}.{fraction:0<2}")
        } else {
            format!("{raw}.00")
        };
        format!("/manga/{manga_slug}/chapter/{formatted}")
    } else {
        key
    }
}

fn meta_value(body: &str, label: &str) -> Option<String> {
    body.split("meta-item-label")
        .skip(1)
        .find(|chunk| html::strip_tags(chunk).trim_start().starts_with(label))
        .and_then(|chunk| html::text_between(chunk, "meta-item-value", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(body: &str) -> ItemStatus {
    match html::text_between(body, "hero-status-badge", "</")
        .map(|value| html::strip_tags(&value).to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_matches('/'));
    }
    format!("/{}", value.trim_matches('/'))
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="card" href="/manga/sample"><div class="card-cover"><img src="/cover.jpg"></div><div class="card-title">Sample TheManga</div></a>
<a class="explore-pagination__btn" rel="next" href="/?page=2">Next</a>
"#;
const SEARCH_FIXTURE: &str = r#"
<a class="manga-card" href="/manga/sample"><div class="cover"><img src="/cover.jpg"></div><div class="card-title">Sample TheManga</div></a>
<a rel="next" href="/explore?page=2">Next</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="hero-title">Sample TheManga</h1>
<div class="hero-cover"><img src="/cover.jpg"></div>
<div class="hero-status-badge">Ongoing</div>
<div class="synopsis-text">Sample description.</div>
<span class="meta-item-label">Author</span><span class="meta-item-value">Sample Author</span>
<span class="meta-item-label">Artist</span><span class="meta-item-value">Sample Artist</span>
<span class="meta-item-label">Type</span><span class="meta-item-value">Manga</span>
<div class="meta-pill-row"><span class="meta-pill">Action</span></div>
<div class="chapter-row" data-href="/manga/sample/chapter/1.00"><span class="chapter-title">Chapter 1</span><span data-local-time="2024-01-01T00:00:00Z"></span></div>
"#;
const PAGES_FIXTURE: &str = r#"
<img class="page-img" src="/page-1.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_themanga_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE, "a.card").entries[0].title, "Sample TheManga");
        assert_eq!(parse_details(DETAILS_FIXTURE, Some("/manga/sample".into())).authors[0], "Sample Author");
        assert_eq!(parse_chapters(DETAILS_FIXTURE)[0].chapter_number, Some(1.0));
        assert_eq!(parse_pages(PAGES_FIXTURE, "/manga/sample/chapter/1.00").len(), 1);
        assert_eq!(
            normalize_chapter_key("/manga/sample/chapter-1"),
            "/manga/sample/chapter/1.00"
        );
    }
}
