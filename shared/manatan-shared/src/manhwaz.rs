use crate::{
    dates, html,
    manga::{self, image_headers},
    sdk::{
        CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
        PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, http,
        source::MangaSource,
    },
    url,
};
use serde_json::{Value, json};
use std::marker::PhantomData;

pub trait ManhwazConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const LANG: &'static str;
    const CONTENT_RATING: &'static str = "safe";
    const AUTHOR_HEADING: &'static str = "author(s)";
    const STATUS_HEADING: &'static str = "status";
    const SEARCH_PATH: &'static str = "search";
}

pub struct ManhwazSource<C>(PhantomData<C>);

impl<C> ManhwazSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: ManhwazConfig> MangaSource for ManhwazSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "popular" {
            C::BASE_URL.to_string()
        } else {
            format!("{}/?page={page}", C::BASE_URL)
        };
        let body = fetch::<C>(&target)?;
        Ok(Paged {
            entries: if listing(&request) == "popular" {
                parse_popular::<C>(&body)
            } else {
                parse_listing::<C>(&body)
            },
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url::<C>(query) {
            return Ok(Paged {
                entries: vec![fetch_details::<C>(&key)?],
                has_next_page: false,
            });
        }

        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{}/{}?s={}&page={page}",
                C::BASE_URL,
                C::SEARCH_PATH.trim_matches('/'),
                url::query_escape(query)
            )
        } else {
            filtered_url::<C>(&request, page)
        };
        let body = fetch::<C>(&target)?;
        Ok(Paged {
            entries: parse_listing::<C>(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        fetch_details::<C>(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch::<C>(&absolute::<C>(&key))?;
        Ok(parse_chapters::<C>(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1".to_string());
        let target = absolute::<C>(&key);
        let body = fetch::<C>(&target)?;
        Ok(parse_pages::<C>(&body, &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute::<C>(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute::<C>(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url::<C>(input) {
            let is_chapter =
                key.contains("chapter") || key.contains("chap-") || key.contains("chuong");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter)
                    .then(|| fetch_details::<C>(&key))
                    .transpose()?,
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

fn client<C: ManhwazConfig>() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::BASE_URL.trim_end_matches('/')))
        .with_origin(C::BASE_URL)
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch<C: ManhwazConfig>(target: &str) -> ExtensionResult<String> {
    client::<C>().get(target).browser_document().send_text()
}

fn fetch_details<C: ManhwazConfig>(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details::<C>(&fetch::<C>(&absolute::<C>(key))?, key))
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn filtered_url<C: ManhwazConfig>(request: &Value, page: u64) -> String {
    let genre = filter(request, "genre")
        .unwrap_or_default()
        .trim_matches('/');
    let mut target = if genre.is_empty() {
        C::BASE_URL.to_string()
    } else {
        format!("{}/{}", C::BASE_URL, genre)
    };
    let mut query = vec![format!("page={page}")];
    if genre.starts_with("genre/") {
        if let Some(order) = filter(request, "orderBy") {
            query.push(format!("m_orderby={}", url::query_escape(order)));
        }
    }
    target.push('?');
    target.push_str(&query.join("&"));
    target
}

fn absolute<C: ManhwazConfig>(value: &str) -> String {
    url::join_url(C::BASE_URL, value)
}

fn key_from_url<C: ManhwazConfig>(input: &str) -> Option<String> {
    input
        .starts_with(C::BASE_URL)
        .then(|| normalize_key::<C>(input))
        .filter(|key| !key.is_empty() && key != "/")
}

fn normalize_key<C: ManhwazConfig>(value: &str) -> String {
    let value = value
        .trim()
        .trim_start_matches(C::BASE_URL.trim_end_matches('/'))
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

fn parse_popular<C: ManhwazConfig>(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .filter(|chunk| chunk.contains("info-item") && chunk.contains("img-item"))
        .filter_map(item_from_chunk::<C>)
        .fold(Vec::new(), push_unique)
}

fn parse_listing<C: ManhwazConfig>(body: &str) -> Vec<CatalogItem> {
    body.split("page-item-detail")
        .skip(1)
        .filter_map(item_from_chunk::<C>)
        .fold(Vec::new(), push_unique)
}

fn item_from_chunk<C: ManhwazConfig>(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key::<C>(&href);
    let title = html::text_between(chunk, "item-summary", "</a>")
        .or_else(|| html::text_between(chunk, "info-item", "</a>"))
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| C::NAME.to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(C::BASE_URL, &image)),
        url: Some(absolute::<C>(&key)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        ..CatalogItem::default()
    })
}

fn parse_details<C: ManhwazConfig>(body: &str, key: &str) -> CatalogItem {
    let mut item = CatalogItem {
        key: normalize_key::<C>(key),
        title: html::text_between(body, "div.post-title h1", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| C::NAME.to_string())),
        cover: html::attr_after(body, "summary_image", "data-src")
            .or_else(|| html::attr_after(body, "summary_image", "data-lazy-src"))
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .map(|image| url::join_url(C::BASE_URL, &image)),
        description: html::text_between(body, "summary__content", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: parse_genres(body),
        authors: summary_value(body, C::AUTHOR_HEADING).into_iter().collect(),
        url: Some(absolute::<C>(key)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    item.artists = item.authors.clone();
    item.status = status_from(
        summary_value(body, C::STATUS_HEADING)
            .as_deref()
            .unwrap_or_default(),
    );
    item
}

fn summary_value(body: &str, heading: &str) -> Option<String> {
    body.split("summary-heading")
        .skip(1)
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_lowercase()
                .contains(&heading.to_lowercase())
        })
        .and_then(|chunk| html::text_between(chunk, "summary-content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_genres(body: &str) -> Vec<String> {
    let block = body.split("genres-content").nth(1).unwrap_or_default();
    block
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("rel=\"tag\"") || chunk.contains("rel='tag'"))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("completed") || lower.contains("hoàn thành") || lower.contains("truyện full")
    {
        ItemStatus::Completed
    } else if lower.contains("ongoing") || lower.contains("đang ra") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters<C: ManhwazConfig>(body: &str) -> Vec<MangaChapter> {
    body.split("wp-manga-chapter")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key::<C>(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute::<C>(&key)),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .and_then(|value| parse_date(&html::strip_tags(&value))),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_date(text: &str) -> Option<i64> {
    dates::parse_ymd(text)
}

fn parse_pages<C: ManhwazConfig>(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("page-break")
        .skip(1)
        .filter_map(image_attr)
        .map(|image| url::join_url(C::BASE_URL, &image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(image_headers(referer)),
            },
            headers: image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| {
            html::attr_after(chunk, "<img", "srcset")
                .map(|value| value.split_whitespace().next().unwrap_or("").to_string())
        })
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"")
        || body.contains("rel='next'")
        || body.contains("pager")
            && (body.contains("next page-numbers") || body.contains("class=\"next\""))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureConfig;

    impl ManhwazConfig for FixtureConfig {
        const NAME: &'static str = "Fixture";
        const BASE_URL: &'static str = "https://fixture.test";
        const LANG: &'static str = "vi";
        const AUTHOR_HEADING: &'static str = "Tác giả";
        const STATUS_HEADING: &'static str = "Trạng thái";
    }

    #[test]
    fn parses_listing_cards() {
        let body = r#"
            <div class="page-item-detail">
              <div class="item-thumb"><a href="/comic-a"><img data-src="/a.jpg"></a></div>
              <div class="item-summary"><a href="/comic-a">Comic A</a></div>
            </div>
        "#;
        let entries = parse_listing::<FixtureConfig>(body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "/comic-a");
        assert_eq!(entries[0].title, "Comic A");
    }

    #[test]
    fn parses_chapters_and_pages() {
        let chapters = parse_chapters::<FixtureConfig>(
            r#"<li class="wp-manga-chapter"><a href="/comic-a/chapter-1">Chapter 1</a></li>"#,
        );
        assert_eq!(chapters[0].key, "/comic-a/chapter-1");

        let pages = parse_pages::<FixtureConfig>(
            r#"<div class="page-break"><img data-src="/p1.jpg"></div>"#,
            "https://fixture.test/comic-a/chapter-1",
        );
        assert_eq!(pages.len(), 1);
    }
}
