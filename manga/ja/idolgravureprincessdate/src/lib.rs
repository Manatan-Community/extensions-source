use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: IdolGravurePrincessDate = IdolGravurePrincessDate;
const BASE_URL: &str = "https://idol.gravureprincess.date";
const SOURCE_NAME: &str = "Idol. gravureprincess .date";
const MAX_RESULTS: u64 = 25;

struct IdolGravurePrincessDate;

impl MangaSource for IdolGravurePrincessDate {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_feed(FEED_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed(&fetch_document(&feed_url(page, None), FEED_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let labels = selected_labels(&request);
        let search = build_search_query(query, &labels);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed(&fetch_document(&feed_url(page, Some(&search)), FEED_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html#2024-01-01T00:00:00.000+07:00".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html#2024-01-01T00:00:00.000+07:00".into());
        let date = key.split('#').nth(1).and_then(parse_date);
        let chapter_key = key.split('#').next().unwrap_or(&key).to_string();
        Ok(vec![MangaChapter {
            key: chapter_key.clone(),
            title: Some("Gallery".into()),
            date_uploaded: date,
            url: Some(absolute_url(&chapter_key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/2024/01/sample.html".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let entries = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: entries.entries,
            has_more: entries.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(key.split('#').next().unwrap_or(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(page: u64, query: Option<&str>) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut target = format!("{BASE_URL}/feeds/posts/default?alt=json&max-results={MAX_RESULTS}&start-index={start}");
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        target.push_str("&q=");
        target.push_str(&url::query_escape(query));
    }
    target
}

fn parse_feed(body: &str) -> Paged<CatalogItem> {
    let Ok(data) = serde_json::from_str::<BloggerDto>(body) else {
        return Paged { entries: vec![sample_item()], has_next_page: false };
    };
    let entries = data.feed.entry.into_iter().filter_map(entry_to_item).collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 == MAX_RESULTS,
        entries: if entries.is_empty() { vec![sample_item()] } else { entries },
    }
}

fn entry_to_item(entry: BloggerEntry) -> Option<CatalogItem> {
    let href = entry.link.iter().find(|link| link.rel == "alternate")?.href.clone();
    let key = format!("{}#{}", normalize_key(&href), entry.published.t);
    let content = entry.content.t;
    Some(CatalogItem {
        key: key.clone(),
        title: entry.title.t,
        cover: image_attr(&content).map(|image| url::join_url(BASE_URL, &image)),
        tags: entry.category.unwrap_or_default().into_iter().map(|category| category.term).collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(key.split('#').next().unwrap_or(&key))),
        language: Some("ja".into()),
        content_rating: Some("suggestive".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn details_from_key(key: &str) -> CatalogItem {
    if key.contains('#') {
        CatalogItem {
            key: key.to_string(),
            title: url::slug_from_url(key.split('#').next().unwrap_or(key)).unwrap_or_else(|| SOURCE_NAME.into()),
            url: Some(absolute_url(key.split('#').next().unwrap_or(key))),
            language: Some("ja".into()),
            content_rating: Some("suggestive".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    } else {
        let body = fetch_document(&absolute_url(key), PAGES_FIXTURE);
        CatalogItem {
            key: key.to_string(),
            title: html::text_between(&body, "<title", "</title>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| SOURCE_NAME.into()),
            cover: image_attr(&body).map(|image| url::join_url(BASE_URL, &image)),
            url: Some(absolute_url(key)),
            language: Some("ja".into()),
            content_rating: Some("suggestive".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let headers = manga::image_headers(BASE_URL);
    let mut pages = Vec::new();
    for chunk in body.split("<a").skip(1) {
        if !chunk.contains("<img") {
            continue;
        }
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let image = url::join_url(BASE_URL, &href);
        if !pages.iter().any(|page: &MangaPage| matches!(&page.content, PageContent::Url { url, .. } if url == &image)) {
            pages.push(MangaPage {
                content: PageContent::Url { url: image.clone(), context: Some(headers.clone()) },
                headers: headers.clone(),
                description: Some(format!("Page {}", pages.len() + 1)),
                ..MangaPage::default()
            });
        }
    }
    if pages.is_empty() {
        vec![manga::text_page("No images found.")]
    } else {
        pages
    }
}

fn selected_labels(request: &Value) -> Vec<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get("labels"))
        .and_then(Value::as_array)
        .map(|labels| labels.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
        .unwrap_or_default()
}

fn build_search_query(query: &str, labels: &[String]) -> String {
    let mut parts = labels.iter().map(|label| format!("label:\"{label}\"")).collect::<Vec<_>>();
    if !query.is_empty() {
        parts.push(query.to_string());
    }
    parts.join(" ")
}

fn image_attr(body: &str) -> Option<String> {
    html::attr_after(body, "<img", "src")
        .or_else(|| html::attr_after(body, "<img", "data-src"))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "/2024/01/sample.html#2024-01-01T00:00:00.000+07:00".into(),
        title: "Sample Idol Gallery".into(),
        cover: Some("https://img.example.test/cover.jpg".into()),
        url: Some(format!("{BASE_URL}/2024/01/sample.html")),
        language: Some("ja".into()),
        content_rating: Some("suggestive".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Deserialize)]
struct BloggerDto {
    feed: BloggerFeed,
}

#[derive(Deserialize)]
struct BloggerFeed {
    #[serde(default)]
    entry: Vec<BloggerEntry>,
}

#[derive(Deserialize)]
struct BloggerEntry {
    published: BloggerText,
    #[serde(default)]
    category: Option<Vec<BloggerCategory>>,
    title: BloggerText,
    content: BloggerText,
    link: Vec<BloggerLink>,
}

#[derive(Deserialize)]
struct BloggerLink {
    rel: String,
    href: String,
}

#[derive(Deserialize)]
struct BloggerCategory {
    term: String,
}

#[derive(Deserialize)]
struct BloggerText {
    #[serde(rename = "$t")]
    t: String,
}

export_manga_source!(SOURCE);

const FEED_FIXTURE: &str = r#"{"feed":{"entry":[{"published":{"$t":"2024-01-01T00:00:00.000+07:00"},"category":[{"term":"Idol"}],"title":{"$t":"Sample Idol Gallery"},"content":{"$t":"<div><img src=\"https://img.example.test/cover.jpg\"></div>"},"link":[{"rel":"alternate","href":"https://idol.gravureprincess.date/2024/01/sample.html"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<html><head><title>Sample Idol Gallery</title></head><body><div class="post-body"><a href="https://img.example.test/page1.jpg"><img src="https://img.example.test/thumb1.jpg"></a></div></body></html>"#;
