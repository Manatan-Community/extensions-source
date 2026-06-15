use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: GalaxScanlator = GalaxScanlator;
const BASE_URL: &str = "https://galaxscanlator.blogspot.com";
const NAME: &str = "GALAX Scans";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const MAX_RESULTS: u64 = 20;
const CHAPTER_RESULTS: u64 = 999_999;
const CHAPTER_LABEL: &str = "Capitulo";

struct GalaxScanlator;

impl MangaSource for GalaxScanlator {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_home_listing(HOME_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            return Ok(parse_feed_listing(&fetch_json(
                &feed_url(page, None, None),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_home_listing(&fetch_document(BASE_URL, HOME_FIXTURE)))
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
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed_listing(&fetch_json(
            &feed_url(page, Some(query), None),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        Ok(parse_chapter_feed(
            &fetch_json(
                &feed_url(1, None, Some(CHAPTER_LABEL)).replace(
                    &format!("max-results={}", MAX_RESULTS + 1),
                    &format!("max-results={CHAPTER_RESULTS}"),
                ),
                CHAPTERS_FIXTURE,
            ),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample-chapter-1.html".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_home_listing(&fetch_document(BASE_URL, HOME_FIXTURE));
        let latest = parse_feed_listing(&fetch_json(&feed_url(1, None, None), LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(page: u64, query: Option<&str>, label: Option<&str>) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut path = format!("{BASE_URL}/feeds/posts/default");
    if let Some(label) = label.filter(|value| !value.is_empty()) {
        path.push_str("/-/");
        path.push_str(&url::query_escape(label));
    }
    let mut params = vec![
        "alt=json".to_string(),
        format!("max-results={}", MAX_RESULTS + 1),
        format!("start-index={start}"),
    ];
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        params.push(format!("q={}", url::query_escape(query)));
    }
    format!("{path}?{}", params.join("&"))
}

fn parse_home_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("PopularPosts2") || chunk.contains("<h3"))
        .filter_map(catalog_from_chunk)
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: false,
        entries,
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h3", "</h3>")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.to_string())),
        cover: image_attr(chunk).map(|image| fix_google_image(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_feed_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry_link(entry).is_some())
        .map(entry_to_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() > MAX_RESULTS as usize,
        entries: entries.into_iter().take(MAX_RESULTS as usize).collect(),
    }
}

fn entry_to_catalog(entry: &Value) -> CatalogItem {
    let href = entry_link(entry).unwrap_or_else(|| format!("{BASE_URL}/p/sample.html"));
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title: entry_title(entry).unwrap_or_else(|| NAME.to_string()),
        cover: entry_thumbnail(entry),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.to_string())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "grid gta-series", "src"))
            .or_else(|| image_attr(body))
            .map(|image| fix_google_image(&image)),
        description: html::text_between(body, "id=\"synopsis\"", "</")
            .or_else(|| html::text_between(body, "post-body", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("rel=\"tag\"")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("completo") || lower.contains("finalizado") {
            ItemStatus::Completed
        } else if lower.contains("hiato") {
            ItemStatus::Hiatus
        } else if lower.contains("ativo") || lower.contains("andamento") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let href = entry_link(entry)?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: entry_title(entry),
                date_uploaded: entry
                    .get("published")
                    .and_then(|value| value.get("$t"))
                    .and_then(Value::as_str)
                    .and_then(parse_feed_date),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo".to_string()),
            url: Some(absolute_url(manga_key)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("reader")
                || chunk.contains("blogger")
                || chunk.contains("bp.blogspot")
                || chunk.contains("googleusercontent")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: fix_google_image(&absolute_url(&image)),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn entry_title(entry: &Value) -> Option<String> {
    entry
        .get("title")
        .and_then(|title| title.get("$t"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn entry_link(entry: &Value) -> Option<String> {
    entry
        .get("link")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))
        .and_then(|link| link.get("href"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn entry_thumbnail(entry: &Value) -> Option<String> {
    entry
        .get("media$thumbnail")
        .and_then(|thumb| thumb.get("url"))
        .and_then(Value::as_str)
        .map(fix_google_image)
        .or_else(|| {
            entry
                .get("content")
                .and_then(|content| content.get("$t"))
                .and_then(Value::as_str)
                .and_then(image_attr)
                .map(|image| fix_google_image(&image))
        })
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn fix_google_image(input: &str) -> String {
    input
        .replace("/s72-c/", "/s1600/")
        .replace("=s72-c", "=s1600")
        .replace("/w72-h72-p-k-no-nu/", "/s1600/")
}

fn parse_feed_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
<div id="PopularPosts2"><article><a href="https://galaxscanlator.blogspot.com/p/sample.html"><img src="https://blogger.googleusercontent.com/img/s72-c/cover.jpg"></a><h3>Sample Galax</h3></article></div>
"#;
const LIST_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Sample Galax"},"link":[{"rel":"alternate","href":"https://galaxscanlator.blogspot.com/p/sample.html"}],"media$thumbnail":{"url":"https://blogger.googleusercontent.com/img/s72-c/cover.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Galax</h1><meta property="og:image" content="https://blogger.googleusercontent.com/img/s72-c/cover.jpg">
<div id="synopsis">Sample description.</div><div class="grid gta-series"><dt>Genre</dt><dd><a rel="tag">Acao</a></dd></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Capitulo 1"},"published":{"$t":"2024-01-01T00:00:00.000Z"},"link":[{"rel":"alternate","href":"https://galaxscanlator.blogspot.com/2024/01/sample-chapter-1.html"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<div id="reader"><img src="https://blogger.googleusercontent.com/img/s1600/page1.jpg"></div>"#;
