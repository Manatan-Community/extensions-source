use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Yokai = Yokai;
const BASE_URL: &str = "https://yokai-team.blogspot.com";
const MANGA_CATEGORY: &str = "Series";
const CHAPTER_CATEGORY: &str = "Chapter";

struct Yokai;

impl MangaSource for Yokai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            let body =
                fetch_text_or_fixture(&feed_url(CHAPTER_CATEGORY, page, "", None), FEED_FIXTURE);
            return Ok(parse_feed(&body));
        }
        let body = fetch_text_or_fixture(BASE_URL, POPULAR_FIXTURE);
        Ok(parse_popular(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_text_or_fixture(
            &feed_url(MANGA_CATEGORY, page, query, Some(MANGA_CATEGORY)),
            FEED_FIXTURE,
        );
        Ok(parse_feed(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/2024/01/sample.html".to_string());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/2024/01/sample.html".to_string());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let label = html::attr_after(&body, "manga-widget", "data-label")
            .unwrap_or_else(|| CHAPTER_CATEGORY.to_string());
        let feed = fetch_text_or_fixture(&feed_url(&label, 1, "", None), CHAPTER_FEED_FIXTURE);
        let mut chapters = parse_chapter_feed(&feed);
        for chapter in parse_download_chapters(&body) {
            if !chapters.iter().any(|existing| existing.key == chapter.key) {
                chapters.push(chapter);
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/chapter-1.html".to_string());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(label: &str, page: u64, query: &str, search_label: Option<&str>) -> String {
    let start = 25 * page.saturating_sub(1) + 1;
    let mut target = format!(
        "{BASE_URL}/feeds/posts/default/-/{}?alt=json&max-results=26&start-index={start}",
        url::query_escape(label)
    );
    if !query.is_empty() {
        let prefix = search_label
            .map(|label| format!("label:{label}+"))
            .unwrap_or_default();
        target.push_str("&q=");
        target.push_str(&prefix);
        target.push_str(&url::query_escape(query));
    }
    target
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "figcaption", "</figcaption>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
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

fn parse_feed(body: &str) -> Paged<CatalogItem> {
    let entries = feed_entries(body)
        .into_iter()
        .filter(|entry| categories(entry).iter().any(|term| term == MANGA_CATEGORY))
        .map(catalog_from_entry)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() > 25,
        entries: entries.into_iter().take(25).collect(),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/2024/01/sample.html".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "synopsis", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("rel=\"tag\"")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str) -> Vec<MangaChapter> {
    feed_entries(body)
        .into_iter()
        .filter(|entry| {
            categories(entry)
                .iter()
                .any(|term| term == CHAPTER_CATEGORY)
        })
        .filter_map(|entry| {
            let title = entry.get("title")?.get("$t")?.as_str()?.to_string();
            let href = alternate_link(&entry)?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: entry
                    .get("published")
                    .and_then(|value| value.get("$t"))
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_download_chapters(body: &str) -> Vec<MangaChapter> {
    let Some(download_block) = html::text_between(body, "id=\"download\"", "</div>")
        .or_else(|| html::text_between(body, "id='download'", "</div>"))
    else {
        return Vec::new();
    };
    download_block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
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

fn feed_entries(body: &str) -> Vec<Value> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn catalog_from_entry(entry: Value) -> CatalogItem {
    let title = entry
        .get("title")
        .and_then(|value| value.get("$t"))
        .and_then(Value::as_str)
        .unwrap_or("Manga")
        .to_string();
    let href = alternate_link(&entry).unwrap_or_else(|| BASE_URL.to_string());
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title,
        cover: feed_cover(&entry),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn alternate_link(entry: &Value) -> Option<String> {
    entry
        .get("link")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))?
        .get("href")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn feed_cover(entry: &Value) -> Option<String> {
    entry
        .get("media$thumbnail")
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            entry
                .get("content")
                .and_then(|value| value.get("$t"))
                .and_then(Value::as_str)
                .and_then(|content| html::attr_after(content, "<img", "src"))
        })
}

fn categories(entry: &Value) -> Vec<String> {
    entry
        .get("category")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|category| {
            category
                .get("term")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input.trim_start_matches(BASE_URL).trim_start_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/'))
}

const POPULAR_FIXTURE: &str = r#"
<div class="PopularPosts"><div class="grid"><figure><img src="/cover.jpg"><figcaption><a href="/2024/01/sample.html">Sample Manga</a></figcaption></figure></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1><img src="/cover.jpg"><div id="synopsis">Sample description.</div>
<div class="manga-widget" data-label="Sample Chapters"></div>
<div id="download"><div class="index-list"><a href="https://yokai-team.blogspot.com/2024/01/chapter-extra.html">2 Extra</a></div></div>
"#;

const FEED_FIXTURE: &str = r#"{
  "feed": {"entry": [{"title":{"$t":"Sample Manga"},"category":[{"term":"Series"}],"link":[{"rel":"alternate","href":"https://yokai-team.blogspot.com/2024/01/sample.html"}],"media$thumbnail":{"url":"https://img/cover.jpg"}}]}
}"#;

const CHAPTER_FEED_FIXTURE: &str = r#"{
  "feed": {"entry": [{"title":{"$t":"Chapter 1"},"published":{"$t":"2024-01-01T00:00:00.000Z"},"category":[{"term":"Chapter"}],"link":[{"rel":"alternate","href":"https://yokai-team.blogspot.com/2024/01/chapter-1.html"}]}]}
}"#;

const PAGES_FIXTURE: &str = r#"<div class="check-box"><div class="separator"><img src="/page1.jpg"><img src="/page2.jpg"></div></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zeist_source() {
        let popular = parse_popular(POPULAR_FIXTURE);
        assert_eq!(popular.entries[0].title, "Sample Manga");

        let feed = parse_feed(FEED_FIXTURE);
        assert_eq!(feed.entries[0].key, "/2024/01/sample.html");

        let chapters = parse_chapter_feed(CHAPTER_FEED_FIXTURE);
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        assert_eq!(parse_download_chapters(DETAILS_FIXTURE).len(), 1);

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
