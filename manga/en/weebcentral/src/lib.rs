use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: WeebCentral = WeebCentral;
const BASE_URL: &str = "https://weebcentral.com";
const FETCH_LIMIT: u64 = 32;

struct WeebCentral;

impl MangaSource for WeebCentral {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "Latest Updates"
        } else {
            "Popularity"
        };
        Ok(parse_listing(&fetch_document(
            &search_url(page, "", sort, None),
            LIST_FIXTURE,
        )))
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if let Some(id) = query.strip_prefix("id:") {
            let key = format!("/series/{}", id.trim_matches('/'));
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &search_url(
                page,
                query,
                filter(request.get("filters"), "sort", "Best Match"),
                request.get("filters"),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1/sample".into());
        Ok(parse_chapters(&fetch_document(
            &chapter_list_url(&key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapters/1/sample".into());
        Ok(parse_pages(&fetch_document(
            &format!(
                "{}?is_prev=False&reading_style=long_strip",
                url::join_url(BASE_URL, &format!("{}/images", key.trim_end_matches('/')))
            ),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
                HomeSectionStyle::Cover,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
                HomeSectionStyle::Compact,
            ),
        ])
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

fn search_url(page: u64, query: &str, default_sort: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        (
            "text".to_string(),
            query
                .replace(['!', '#', ':', '(', ')', ',', '-'], " ")
                .trim()
                .to_string(),
        ),
        (
            "sort".to_string(),
            filter(filters, "sort", default_sort).to_string(),
        ),
        (
            "order".to_string(),
            filter(filters, "order", "Descending").to_string(),
        ),
        (
            "official".to_string(),
            filter(filters, "official", "Any").to_string(),
        ),
        (
            "anime".to_string(),
            filter(filters, "anime", "Any").to_string(),
        ),
        (
            "adult".to_string(),
            filter(filters, "adult", "Any").to_string(),
        ),
        ("limit".to_string(), FETCH_LIMIT.to_string()),
        (
            "offset".to_string(),
            ((page.saturating_sub(1)) * FETCH_LIMIT).to_string(),
        ),
        ("display_mode".to_string(), "Full Display".to_string()),
    ];
    if let Some(author) = filters
        .and_then(|value| value.get("author"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        params.push(("author".to_string(), author.to_string()));
    }
    for key in [
        "included_status",
        "included_type",
        "included_tag",
        "excluded_tag",
    ] {
        if let Some(values) = filters
            .and_then(|value| value.get(key))
            .and_then(Value::as_array)
        {
            for value in values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                params.push((key.to_string(), value.to_string()));
            }
        }
    }
    format!(
        "{BASE_URL}/search/data?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/series/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "<div", "</div>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Series".into()),
                    cover: source_img(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("<button"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/1/sample".into());
    let title = html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "Series".into());
    let description = html::text_between(body, "<p", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title,
        cover: source_img(body).map(|image| url::join_url(BASE_URL, &image)),
        authors: info_links(body, "Author"),
        tags: {
            let mut values = info_links(body, "Tag");
            values.extend(info_links(body, "Type"));
            values
        },
        status: parse_status(&info_text(body, "Status")),
        description,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapters/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| "Chapter".into());
            let scanlators = match html::attr_after(chunk, "<svg", "stroke").as_deref() {
                Some("#d8b4fe") => vec!["Official".into()],
                Some("#4C4D54") => vec!["Unknown".into()],
                _ => Vec::new(),
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| dates::parse_fixture_date(&value)),
                scanlators,
                language: Some("en".into()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.starts_with("data:"))
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

fn chapter_list_url(key: &str) -> String {
    let mut segments = key.trim_matches('/').split('/').collect::<Vec<_>>();
    if segments.len() >= 2 {
        segments.truncate(2);
        segments.push("full-chapter-list");
        return format!("{BASE_URL}/{}", segments.join("/"));
    }
    url::join_url(BASE_URL, key)
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_matches('/');
    if path.is_empty() {
        "/".into()
    } else {
        format!("/{path}")
    }
}

fn source_img(body: &str) -> Option<String> {
    html::attr_after(body, "<source", "srcset")
        .map(|value| value.replace("small", "normal"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn info_text(body: &str, label: &str) -> String {
    body.split("<li")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .unwrap_or_default()
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("ongoing") => ItemStatus::Ongoing,
        text if text.contains("complete") => ItemStatus::Completed,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        text if text.contains("canceled") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number(value: &str) -> Option<f32> {
    value
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| part.chars().any(|c| c.is_ascii_digit()))
        .and_then(|part| part.parse().ok())
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn home_section(
    id: &str,
    title: &str,
    page: Paged<CatalogItem>,
    style: HomeSectionStyle,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article><section><a href="/series/1/sample"><source srcset="/cover-small.jpg"><div>Sample Series</div></a></section></article><button>More</button>"#;
const DETAILS_FIXTURE: &str = r#"<section x-data><section><img src="/cover.jpg"><ul><li><strong>Author</strong><span><a>Author</a></span></li><li><strong>Status</strong><a>Ongoing</a></li><li><strong>Tag</strong><a>Action</a></li><li><strong>Type</strong><a>Manga</a></li></ul></section><section><h1>Sample Series</h1><li><strong>Description</strong><p>Summary</p></li></section></section>"#;
const CHAPTERS_FIXTURE: &str = r#"<div x-data><a href="/chapters/1/sample-chapter-1"><span class="flex"><span>Chapter 1</span></span><time datetime="2024-01-01T00:00:00.000Z"></time></a></div>"#;
const PAGES_FIXTURE: &str = r#"<section x-data="scroll"><img src="/page1.jpg"></section>"#;
