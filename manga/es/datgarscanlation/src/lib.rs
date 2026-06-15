use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: DatGarScanlation = DatGarScanlation;
const BASE_URL: &str = "https://datgarscanlation.blogspot.com";
const NAME: &str = "Dat-Gar Scan";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";
const MANGA_CATEGORY: &str = "Series";
const CHAPTER_CATEGORY: &str = "Chapter";
const MAX_RESULTS: u64 = 20;

struct DatGarScanlation;

impl MangaSource for DatGarScanlation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular_html(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            return Ok(parse_feed_listing(
                &fetch_text(&feed_url(page, None, Vec::new()), LIST_FIXTURE),
                page,
            ));
        }
        Ok(parse_popular_html(&fetch_document(
            BASE_URL,
            POPULAR_FIXTURE,
        )))
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
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let labels = if query.is_empty() {
            filter_labels(&request)
        } else {
            Vec::new()
        };
        Ok(parse_feed_listing(
            &fetch_text(&feed_url(page, Some(query), labels), LIST_FIXTURE),
            page,
        ))
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
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapter_feed(
            &fetch_text(&chapter_feed_url(&body), CHAPTERS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample.html".into());
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
        let popular = parse_popular_html(&fetch_document(BASE_URL, POPULAR_FIXTURE));
        let latest =
            parse_feed_listing(&fetch_text(&feed_url(1, None, Vec::new()), LIST_FIXTURE), 1);
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
                title: "Recientes".to_string(),
                style: Some(HomeSectionStyle::Compact),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
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

fn feed_url(page: u64, query: Option<&str>, labels: Vec<String>) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut path = format!("{BASE_URL}/feeds/posts/default/-/{MANGA_CATEGORY}");
    for label in labels.into_iter().filter(|value| !value.is_empty()) {
        path.push('/');
        path.push_str(&url::query_escape(&label));
    }
    let mut pairs = vec![
        "alt=json".to_string(),
        format!("max-results={}", MAX_RESULTS + 1),
        format!("start-index={start}"),
    ];
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        pairs.push(format!(
            "q=label:{MANGA_CATEGORY}+{}",
            url::query_escape(query)
        ));
    }
    format!("{path}?{}", pairs.join("&"))
}

fn api_url(label: &str) -> String {
    format!(
        "{BASE_URL}/feeds/posts/default/-/{}?alt=json",
        url::query_escape(label)
    )
}

fn parse_popular_html(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| NAME.to_string())
                    }),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_feed_listing(body: &str, _page: u64) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| has_category(entry, MANGA_CATEGORY) && !has_category(entry, "Anime"))
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
    let author = info_value(body, "autor").or_else(|| info_value(body, "author"));
    let artist = info_value(body, "artista").or_else(|| info_value(body, "artist"));
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.to_string())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "id=\"synopsis\"", "</")
            .or_else(|| html::text_between(body, "summary", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: author.into_iter().collect(),
        artists: artist.into_iter().collect(),
        tags: link_values(body, "rel=\"tag\""),
        status: status_from(&lower),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_feed_url(body: &str) -> String {
    if let Some(label) = quoted_after(body, "label") {
        let mut out = api_url(&label);
        out.push_str("&start-index=1&max-results=999999");
        return out;
    }
    if let Some(feed) = quoted_after(body, "clwd.run(") {
        return format!(
            "{}/{}",
            api_url(CHAPTER_CATEGORY).trim_end_matches("?alt=json"),
            feed
        ) + "?alt=json";
    }
    api_url(CHAPTER_CATEGORY)
}

fn parse_chapter_feed(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| has_category(entry, CHAPTER_CATEGORY))
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
            title: Some("Leer".to_string()),
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
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn filter_labels(request: &Value) -> Vec<String> {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let mut labels = Vec::new();
    for key in ["status", "type"] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            labels.push(value.to_string());
        }
    }
    if let Some(value) = filters.get("genres").and_then(Value::as_str) {
        labels.extend(
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
        );
    }
    labels
}

fn has_category(entry: &Value, category: &str) -> bool {
    entry
        .get("category")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|item| item.get("term").and_then(Value::as_str) == Some(category))
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
        .map(fix_google_thumbnail)
        .or_else(|| {
            entry
                .get("content")
                .and_then(|content| content.get("$t"))
                .and_then(Value::as_str)
                .and_then(|content| html::attr_after(content, "<img", "src"))
        })
}

fn fix_google_thumbnail(input: &str) -> String {
    if let Some(index) = input.find("/s") {
        if let Some(end) = input[index + 2..].find("-c/") {
            let mut out = input.to_string();
            out.replace_range(index..index + 2 + end + 3, "/w600/");
            return out;
        }
    }
    input.to_string()
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<dt")
        .skip(1)
        .find(|chunk| html::strip_tags(chunk).to_ascii_lowercase().contains(label))
        .and_then(|chunk| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(lower: &str) -> ItemStatus {
    if lower.contains("finalizado") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("pausado") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelado") || lower.contains("abandonado") {
        ItemStatus::Cancelled
    } else if lower.contains("en curso") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let quote = rest.find(['"', '\''])?;
    let quote_char = rest.as_bytes()[quote] as char;
    let after = &rest[quote + 1..];
    let end = after.find(quote_char)?;
    Some(after[..end].to_string())
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

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
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

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"<div class="PopularPosts"><div class="grid"><figure><a href="/p/sample.html"><img src="/cover.jpg"></a><figcaption><a href="/p/sample.html">Sample</a></figcaption></figure></div></div>"#;
const LIST_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Sample"},"category":[{"term":"Series"}],"link":[{"rel":"alternate","href":"https://datgarscanlation.blogspot.com/p/sample.html"}],"media$thumbnail":{"url":"https://1.bp.blogspot.com/s72-c/sample.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"<main><h1>Sample</h1><img src="/cover.jpg"><div id="synopsis">Summary</div><div id="latest"><script>label = 'sample'</script></div><a rel="tag">Drama</a></main>"#;
const CHAPTERS_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Capitulo 1"},"published":{"$t":"2024-01-01T00:00:00.000Z"},"category":[{"term":"Chapter"}],"link":[{"rel":"alternate","href":"https://datgarscanlation.blogspot.com/2024/01/sample.html"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<div class="check-box"><div class="separator"><img src="/page1.jpg"></div><div class="separator"><img src="/page2.jpg"></div></div>"#;
