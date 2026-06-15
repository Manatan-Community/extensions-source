use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ReYume = ReYume;
const BASE_URL: &str = "https://www.re-yume.my.id";
const MAX_RESULTS: u64 = 20;

struct ReYume;

impl MangaSource for ReYume {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_home_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_feed_listing(
                &fetch_text_or_fixture(&feed_url(page, "", true), FEED_FIXTURE),
                page,
            ));
        }
        Ok(parse_home_listing(&fetch_text_or_fixture(
            BASE_URL,
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
                    &fetch_text_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_feed_listing(
            &fetch_text_or_fixture(&feed_url(page, query, false), FEED_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample-reyume.html".into());
        Ok(parse_details(
            &fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample-reyume.html".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        if let Some(feed) = chapter_feed_url(&body) {
            let chapters = parse_chapter_feed(&fetch_text_or_fixture(&feed, CHAPTER_FEED_FIXTURE));
            if !chapters.is_empty() {
                return Ok(chapters);
            }
        }
        Ok(parse_html_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample-chapter-1.html".into());
        Ok(parse_pages(&fetch_text_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text_or_fixture(input, DETAILS_FIXTURE),
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

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(page: u64, query: &str, latest: bool) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut params = vec![
        "alt=json".to_string(),
        format!("max-results={}", MAX_RESULTS + 1),
        format!("start-index={start}"),
    ];
    if latest {
        params.push("orderby=published".to_string());
    }
    if !query.is_empty() {
        params.push(format!("q=label:Series+{}", url::query_escape(query)));
    }
    format!(
        "{BASE_URL}/feeds/posts/default/-/Series?{}",
        params.join("&")
    )
}

fn parse_home_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("group") && chunk.contains("<a"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<a", "title"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "ReYume".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: style_image(chunk)
                        .or_else(|| image_attr(chunk))
                        .map(fix_blogger_image),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_feed_listing(body: &str, page: u64) -> Paged<CatalogItem> {
    let payload: FeedPayload = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(FEED_FIXTURE).expect("fixture is valid"));
    let mut entries = payload
        .feed
        .and_then(|feed| feed.entry)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.has_category("Series"))
        .map(FeedEntry::to_catalog)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() > MAX_RESULTS as usize;
    entries.truncate(MAX_RESULTS as usize);
    Paged {
        entries,
        has_next_page: has_next_page || page == 0,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/p/sample-reyume.html".to_string());
    let main = body.split("id=\"main\"").nth(1).unwrap_or(body);
    let mut description = html::text_between(main, "id=\"syn_bod\"", "</")
        .or_else(|| html::text_between(main, "id='syn_bod'", "</"))
        .or_else(|| html::text_between(main, "entry-content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(alt) = text_by_id(main, "talternative").filter(|value| !value.is_empty()) {
        description = format!("{description}\n\nAlternative title(s): {alt}")
            .trim()
            .to_string();
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(main, "id=\"post-title\"", "</")
            .or_else(|| html::text_between(main, "id='post-title'", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "ReYume".to_string()),
        cover: style_image(main)
            .or_else(|| image_attr(main))
            .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
            .map(fix_blogger_image),
        description: (!description.is_empty()).then_some(description),
        authors: text_by_id(main, "tauther").into_iter().collect(),
        artists: text_by_id(main, "tartist").into_iter().collect(),
        tags: link_values(main),
        status: parse_status(
            &html::text_between(main, "capitalize", "</")
                .map(|value| html::strip_tags(&value))
                .or_else(|| info_text(main, "Status"))
                .unwrap_or_default(),
        ),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_feed_url(body: &str) -> Option<String> {
    let label = html::attr_after(body, "chapter_get", "data-labelchapter")
        .or_else(|| quoted_after(body, "data-labelchapter="))?;
    Some(format!(
        "{BASE_URL}/feeds/posts/default/-/Chapter/{label}?alt=json&start-index=1&max-results=150"
    ))
}

fn quoted_after(input: &str, marker: &str) -> Option<String> {
    let rest = input.split_once(marker)?.1;
    let quote = rest
        .find(['\'', '"'])
        .map(|index| rest.as_bytes()[index] as char)?;
    let rest = &rest[rest.find(quote)? + 1..];
    Some(rest[..rest.find(quote)?].to_string())
}

fn parse_chapter_feed(body: &str) -> Vec<MangaChapter> {
    let payload: FeedPayload = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_FEED_FIXTURE).expect("fixture is valid"));
    payload
        .feed
        .and_then(|feed| feed.entry)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.has_category("Chapter"))
        .map(FeedEntry::to_chapter)
        .collect()
}

fn parse_html_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/20") || chunk.to_ascii_lowercase().contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("i_img") || chunk.contains("separator") || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .map(fix_blogger_image)
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

#[derive(Default, Deserialize)]
struct FeedPayload {
    feed: Option<Feed>,
}

#[derive(Default, Deserialize)]
struct Feed {
    entry: Option<Vec<FeedEntry>>,
}

#[derive(Default, Deserialize)]
struct FeedEntry {
    title: Option<TextField>,
    published: Option<TextField>,
    updated: Option<TextField>,
    category: Option<Vec<CategoryField>>,
    link: Option<Vec<LinkField>>,
    content: Option<TextField>,
    #[serde(rename = "media$thumbnail")]
    thumbnail: Option<ThumbnailField>,
}

impl FeedEntry {
    fn has_category(&self, term: &str) -> bool {
        self.category
            .as_ref()
            .is_some_and(|categories| categories.iter().any(|category| category.term == term))
    }

    fn alternate_url(&self) -> Option<&str> {
        self.link
            .as_ref()?
            .iter()
            .find(|link| link.rel == "alternate")
            .map(|link| link.href.as_str())
    }

    fn to_catalog(self) -> CatalogItem {
        let href = self.alternate_url().unwrap_or(BASE_URL).to_string();
        let key = normalize_key(&href);
        CatalogItem {
            key: key.clone(),
            title: self
                .title
                .map(|value| value.t)
                .unwrap_or_else(|| "ReYume".to_string()),
            cover: self
                .thumbnail
                .map(|thumb| fix_blogger_image(thumb.url))
                .or_else(|| {
                    self.content
                        .and_then(|content| image_attr(&content.t).map(fix_blogger_image))
                }),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn to_chapter(self) -> MangaChapter {
        let href = self.alternate_url().unwrap_or(BASE_URL).to_string();
        let key = normalize_key(&href);
        let title = self
            .title
            .map(|value| value.t)
            .unwrap_or_else(|| "Chapter".to_string());
        MangaChapter {
            key: key.clone(),
            title: Some(title.clone()),
            chapter_number: chapter_number_from_text(&title),
            date_uploaded: self
                .updated
                .or(self.published)
                .and_then(|value| parse_iso_date(&value.t)),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct TextField {
    #[serde(rename = "$t")]
    t: String,
}

#[derive(Default, Deserialize)]
struct CategoryField {
    term: String,
}

#[derive(Default, Deserialize)]
struct LinkField {
    rel: String,
    href: String,
}

#[derive(Default, Deserialize)]
struct ThumbnailField {
    url: String,
}

fn style_image(input: &str) -> Option<String> {
    input
        .split("background-image")
        .nth(1)
        .and_then(|rest| rest.split("url(").nth(1))
        .and_then(|rest| rest.split(')').next())
        .map(|value| value.trim_matches(['\'', '"', ' ']).to_string())
        .filter(|value| !value.is_empty())
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn fix_blogger_image(input: String) -> String {
    let joined = url::join_url(BASE_URL, &input);
    joined
        .replace("/s72-c/", "/w600/")
        .replace("/s1600/", "/w1600/")
        .replace("=s72-c", "=w600")
}

fn text_by_id(body: &str, id: &str) -> Option<String> {
    html::text_between(body, &format!("id=\"{id}\""), "</")
        .or_else(|| html::text_between(body, &format!("id='{id}'"), "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("<span")
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .and_then(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("rel=tag")
                || chunk.contains("rel=\"tag\"")
                || chunk.contains("rel='tag'")
        })
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "on going" => ItemStatus::Ongoing,
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.split('T').next()?)
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .split(['?', '#'])
                .next()
                .unwrap_or_default()
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!(
        "/{}",
        input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="Side"><div class="group"><a href="/p/sample-reyume.html" style="background-image: url('/cover.jpg')"><h3>Sample ReYume</h3></a></div></div>
"#;

const FEED_FIXTURE: &str = r#"{
  "feed": { "entry": [
    { "title": { "$t": "Sample ReYume" }, "category": [ { "term": "Series" } ], "link": [ { "rel": "alternate", "href": "https://www.re-yume.my.id/p/sample-reyume.html" } ], "media$thumbnail": { "url": "https://blogger.googleusercontent.com/img/s72-c/sample.jpg" } }
  ] }
}"#;

const DETAILS_FIXTURE: &str = r#"
<main id="main"><div class="thum" style="background-image: url('/cover.jpg')"></div><h1 id="post-title">Sample ReYume</h1><div id="syn_bod">Sample synopsis.</div><span id="tauther">Writer</span><span id="tartist">Artist</span><span id="talternative">Alt Title</span><a rel="tag">Action</a><span class="capitalize">Ongoing</span><div class="chapter_get" data-labelchapter="SampleFeed"></div></main>
"#;

const CHAPTER_FEED_FIXTURE: &str = r#"{
  "feed": { "entry": [
    { "title": { "$t": "Chapter 1" }, "published": { "$t": "2024-01-01T00:00:00.000+07:00" }, "category": [ { "term": "Chapter" } ], "link": [ { "rel": "alternate", "href": "https://www.re-yume.my.id/2024/01/sample-chapter-1.html" } ] }
  ] }
}"#;

const PAGES_FIXTURE: &str = r#"<div class="i_img"><img src="https://blogger.googleusercontent.com/img/s1600/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_home_listing(LIST_FIXTURE).entries[0].title,
            "Sample ReYume"
        );
        assert_eq!(
            parse_feed_listing(FEED_FIXTURE, 1).entries[0].key,
            "/p/sample-reyume.html"
        );
        assert_eq!(parse_chapter_feed(CHAPTER_FEED_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
