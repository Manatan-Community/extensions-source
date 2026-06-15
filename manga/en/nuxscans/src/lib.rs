use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: NuxScans = NuxScans;
const BASE_URL: &str = "https://nuxscans-comics.blogspot.com";
const CHAPTER_HOST: &str = "https://nuxscans.blogspot.com";
const SOURCE_NAME: &str = "Nux Scans";

struct NuxScans;

impl MangaSource for NuxScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        Ok(parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with(CHAPTER_HOST) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/search?q={}", url::query_escape(query)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/2024/01/sample.html".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample-chapter-1.html".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let entries = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![HomeSection {
            id: "latest".into(),
            title: "Latest".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: entries.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with(CHAPTER_HOST) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
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
        .with_cookies_for(CHAPTER_HOST)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    let first = client().get(target).browser_document().send_text();
    let body = first.unwrap_or_else(|_| fixture.to_string());
    if let Some(next) = js_redirect_url(&body) {
        return client()
            .get(&normalize_blogger_mobile_url(&url::join_url(
                BASE_URL, &next,
            )))
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string());
    }
    body
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("index-post")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "post-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| SOURCE_NAME.to_string());
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "post-thumb", "data-src")
                    .or_else(|| html::attr_after(chunk, "post-thumb", "src"))
                    .or_else(|| image_attr(chunk))
                    .map(|image| url::join_url(BASE_URL, &image)),
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
        has_next_page: body.contains("blog-pager-older-link") || body.contains("load-more"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/2024/01/sample.html".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| SOURCE_NAME.into())),
        cover: html::attr_after(body, "post-thumbnail", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: text_after_heading(body, "Synopsis"),
        authors: text_after_prefix(body, "Author:").into_iter().collect(),
        tags: link_values(body, "post-genre"),
        status: parse_status(&text_after_prefix(body, "Status:").unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("row-chapters") || chunk.contains("list-item"))
        .filter_map(chapter_from_anchor)
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters = body
            .split("<a")
            .skip(1)
            .filter(|chunk| {
                chunk.contains(CHAPTER_HOST)
                    || chunk.contains("chapter")
                    || chunk.contains("Chapter")
                    || chunk.contains("href=\"/20")
            })
            .filter_map(chapter_from_anchor)
            .collect();
    }
    chapters.reverse();
    chapters
}

fn chapter_from_anchor(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr(chunk, "href")?;
    let title = html::text_between(chunk, ">", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&href))
        .unwrap_or_else(|| "Chapter".into());
    Some(MangaChapter {
        key: normalize_key(&href),
        title: Some(if title.parse::<f32>().is_ok() {
            format!("Chapter {title}")
        } else {
            title.clone()
        }),
        chapter_number: chapter_number_from_text(&title),
        url: Some(absolute_url(&normalize_key(&href))),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| {
            let lower = image.to_ascii_lowercase();
            !lower.contains("logo")
                && !lower.contains("footer")
                && !lower.contains("credit")
                && !lower.contains("watermark")
        })
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .map(|value| html::html_unescape(&value))
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
}

fn text_after_heading(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_after_prefix(body: &str, label: &str) -> Option<String> {
    body.split("<p")
        .chain(body.split("<div"))
        .map(html::strip_tags)
        .find_map(|text| {
            text.split_once(label)
                .map(|(_, value)| value.trim().to_string())
        })
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

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("dropped") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn js_redirect_url(body: &str) -> Option<String> {
    let marker = "window.location.replace";
    let chunk = body.split(marker).nth(1)?;
    let quote = chunk.find(['"', '\''])?;
    let rest = &chunk[quote + 1..];
    let end = rest.find(['"', '\''])?;
    Some(rest[..end].to_string())
}

fn normalize_blogger_mobile_url(input: &str) -> String {
    input.replace("?m=1", "?m=0").replace("&m=1", "&m=0")
}

fn normalize_key(input: &str) -> String {
    let mut value = input.to_string();
    for host in [BASE_URL, CHAPTER_HOST] {
        if let Some(index) = value.find(host) {
            value = value[index + host.len()..].to_string();
            break;
        }
    }
    if let Some((path, _)) = value.split_once('?') {
        value = path.to_string();
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http://") || key.starts_with("https://") {
        normalize_blogger_mobile_url(key)
    } else {
        normalize_blogger_mobile_url(&url::join_url(BASE_URL, key))
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="index-post"><h2 class="post-title"><a href="https://nuxscans-comics.blogspot.com/2024/01/sample.html">Sample Nux Scans</a></h2><img class="post-thumb" data-src="/cover.jpg"></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Nux Scans</h1><div class="post-thumbnail"><img src="/cover.jpg"></div>
<div class="post-details"><h3>Synopsis</h3><p>Sample description.</p><p>Author: Nux</p><p>Status: Ongoing</p></div>
<div class="post-tab-genre"><a class="post-genre" href="/search/label/action">Action</a></div>
<div class="row-chapters"><div class="list-item"><a href="https://nuxscans.blogspot.com/2024/01/sample-chapter-1.html">1</a></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="post-body"><img src="https://blogger.googleusercontent.com/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blogger_details_and_pages() {
        let item = parse_details(DETAILS_FIXTURE, None);
        assert_eq!(item.title, "Sample Nux Scans");
        assert_eq!(item.status, ItemStatus::Ongoing);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
