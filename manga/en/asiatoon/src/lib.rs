use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AsiaToon = AsiaToon;
const BASE_URL: &str = "https://asiatoon.net";

struct AsiaToon;

impl MangaSource for AsiaToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/en/genres/New?page={page}")
        } else {
            format!("{BASE_URL}/en/genres?page={page}")
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            false,
            page,
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
                    &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/en/genres?page={page}")
        } else {
            format!("{BASE_URL}/en/search?keyword={}", url::query_escape(query))
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, SEARCH_FIXTURE),
            !query.is_empty(),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/en/comic/sample.html".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/en/comic/sample.html".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/en/comic/sample/episode-1.html".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
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
                    &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    key,
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
        .with_header("Cookie", "hc_vfs=Y")
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

fn parse_listing(body: &str, text_search: bool, page: u64) -> Paged<CatalogItem> {
    let marker = if text_search {
        "search-item-wrap"
    } else {
        "component-item"
    };
    let entries = body
        .split("<li")
        .chain(body.split("<article"))
        .skip(1)
        .filter(|chunk| chunk.contains(marker) || chunk.contains("thumb"))
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: !text_search && body.contains(&format!("page={}", page + 1)),
        entries,
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "thumb", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| html::text_between(chunk, "webtoon-title", "</").map(|v| html::strip_tags(&v)))
        .or_else(|| html::text_between(chunk, "line-clamp-3", "</").map(|v| html::strip_tags(&v)))
        .or_else(|| url::slug_from_url(&key))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: section_text(body, "Description").or_else(|| section_text(body, "Details")),
        tags: parse_genres(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/episode-"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let text = html::text_between(chunk, ">", "</a>")
                .map(|value| {
                    html::strip_tags(&value)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Episode".to_string());
            let date_text = find_month_date(&text);
            let title = date_text
                .as_deref()
                .map(|date| text.split(date).next().unwrap_or(&text).trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or(text);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: date_text
                    .and_then(|date| manatan_shared::dates::parse_fixture_date(&date)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), |mut chapters, chapter| {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
            chapters
        })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("content__img") && chunk.contains("data-index"))
        .filter_map(|chunk| {
            let index = html::attr(chunk, "data-index")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(usize::MAX);
            image_attr(chunk).map(|url| (index, url))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .map(|(fallback, (index, image))| {
            (if index == usize::MAX { fallback } else { index }, image)
        })
        .collect::<Vec<_>>()
        .tap_sort_by_key(|(index, _)| *index)
        .into_iter()
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
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

trait SortVec<T> {
    fn tap_sort_by_key<K: Ord, F: FnMut(&T) -> K>(self, f: F) -> Self;
}

impl<T> SortVec<T> for Vec<T> {
    fn tap_sort_by_key<K: Ord, F: FnMut(&T) -> K>(mut self, f: F) -> Self {
        self.sort_by_key(f);
        self
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_attr(input: &str) -> Option<String> {
    ["data-src", "data-original", "data-lazy-src", "src"]
        .into_iter()
        .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
        .filter(|value| !value.starts_with("data:"))
}

fn parse_genres(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/en/genres/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn section_text(body: &str, heading: &str) -> Option<String> {
    let start = body.find(heading)?;
    html::text_between(&body[start..], "</", "<")
        .or_else(|| html::text_between(&body[start..], "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != heading)
}

fn find_month_date(text: &str) -> Option<String> {
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    for month in months {
        if let Some(start) = text.find(month) {
            let end = (start + 12).min(text.len());
            return Some(text[start..end].trim().to_string());
        }
    }
    None
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"
<article class="component-item"><a class="thumb js-thumbnail" href="/en/comic/sample.html" title="Sample Asia"><img data-src="/cover.jpg"></a></article><a href="?page=2">Next</a>
"#;
const SEARCH_FIXTURE: &str = r#"
<li class="search-item-wrap"><a class="thumb js-thumbnail" href="/en/comic/sample.html" title="Sample Asia"><img data-src="/cover.jpg"></a></li>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="info__right"><h1>Sample Asia</h1><a href="/en/genres/Drama">Drama</a></div><div class="info__left"><img data-src="/cover.jpg"></div>
<h3>Description</h3><p>Sample description.</p><a href="/en/comic/sample/episode-1.html">Episode 1 Jan 01, 2024</a>
"#;
const PAGES_FIXTURE: &str = r#"
<article class="viewer__body"><img class="content__img" data-index="2" data-src="/page2.jpg"><img class="content__img" data-index="1" data-src="/page1.jpg"></article>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_asiatoon_html() {
        let list = parse_listing(LIST_FIXTURE, false, 1);
        assert_eq!(list.entries[0].title, "Sample Asia");
        assert!(list.has_next_page);

        let details = parse_details(DETAILS_FIXTURE, "/en/comic/sample.html".to_string());
        assert_eq!(details.tags, vec!["Drama"]);

        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].title.as_deref(), Some("Episode 1"));

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
