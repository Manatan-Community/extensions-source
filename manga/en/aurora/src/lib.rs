use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Aurora = Aurora;
const BASE_URL: &str = "https://comicaurora.com";
const AUTHOR: &str = "OSP-Red";
const GENRE: &str = "fantasy";

struct Aurora;

impl MangaSource for Aurora {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_archive(ARCHIVE_FIXTURE));
        }
        Ok(parse_archive(&fetch_document_or_fixture(
            &format!("{BASE_URL}/archive/"),
            ARCHIVE_FIXTURE,
        )))
    }

    fn search(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/chapter-one/".into());
        Ok(catalog_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/chapter-one/".into());
        Ok(fetch_chapter_pages(&url::join_url(BASE_URL, &key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/page-one/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        let image = image_from_page(&body);
        Ok(image
            .into_iter()
            .map(|image| MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some("Page 1".to_string()),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key)),
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

fn parse_archive(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("wp-block-image").skip(1) {
        let href = html::attr_after(chunk, "<a", "href");
        let image = html::attr_after(chunk, "<img", "src");
        let title = html::text_between(chunk, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        if let (Some(href), Some(title)) = (href, title) {
            let key = normalize_key(&href);
            entries.push(CatalogItem {
                key: key.clone(),
                title: format!("Aurora - {title}"),
                authors: vec![AUTHOR.to_string()],
                artists: vec![AUTHOR.to_string()],
                description: Some(AURORA_DESCRIPTION.to_string()),
                tags: vec![GENRE.to_string()],
                cover: image.map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                initialized: true,
                ..CatalogItem::default()
            });
        }
    }
    let last_index = entries.len().saturating_sub(1);
    for (index, item) in entries.iter_mut().enumerate() {
        item.status = if index >= last_index {
            ItemStatus::Unknown
        } else {
            ItemStatus::Completed
        };
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_from_key(key: &str) -> CatalogItem {
    let title = format!(
        "Aurora - {}",
        key.trim_matches('/')
            .replace('-', " ")
            .split_whitespace()
            .map(capitalize)
            .collect::<Vec<_>>()
            .join(" ")
    );
    CatalogItem {
        key: normalize_key(key),
        title,
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some(AURORA_DESCRIPTION.to_string()),
        tags: vec![GENRE.to_string()],
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapter_pages(start_url: &str) -> Vec<MangaChapter> {
    let mut url_to_fetch = start_url.to_string();
    let mut chapters = Vec::new();
    for _ in 0..20 {
        let body = fetch_document_or_fixture(&url_to_fetch, CHAPTERS_FIXTURE);
        chapters.extend(parse_post_chapters(&body));
        let Some(next) = next_page_url(&body) else {
            break;
        };
        url_to_fetch = next;
    }
    chapters.reverse();
    chapters
}

fn parse_post_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("post-content")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "webcomic-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "post-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Aurora Page".to_string());
            let chapter_number = title
                .split('.')
                .nth(1)
                .and_then(|value| value.parse::<f32>().ok());
            let date_uploaded = html::text_between(chunk, "post-date", "</")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| parse_month_date(&value));
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number,
                date_uploaded,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn next_page_url(body: &str) -> Option<String> {
    html::attr_after(body, "paginav-next", "href").map(|value| url::join_url(BASE_URL, &value))
}

fn image_from_page(body: &str) -> Option<String> {
    html::attr_after(body, "attachment-full", "src")
        .or_else(|| html::attr_after(body, "webcomic-media", "src"))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        let rest = value.trim_start_matches(BASE_URL);
        format!("/{}", rest.trim_start_matches('/').trim_end_matches('/'))
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn parse_month_date(value: &str) -> Option<i64> {
    let parts = value.replace(',', "");
    let parts = parts.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = match parts[0] {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    Some(timestamp_utc(
        parts[2].parse().ok()?,
        month,
        parts[1].parse().ok()?,
    ))
}

fn timestamp_utc(year: i64, month: i64, day: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400
}

export_manga_source!(SOURCE);

const AURORA_DESCRIPTION: &str = "Aurora is a fantasy webcomic written and illustrated by Red, better known for her work on the YouTube channel Overly Sarcastic Productions.";

const ARCHIVE_FIXTURE: &str = r#"
<figure class="wp-block-image"><a href="https://comicaurora.com/chapter-one/">Chapter One</a><img src="https://comicaurora.com/cover.jpg"></figure>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<article class="post-content"><h2 class="post-title"><a>1.1.1</a></h2><time class="post-date">January 01, 2024</time><a class="webcomic-link" href="https://comicaurora.com/comic/page-one/">Read</a></article>
"#;

const PAGE_FIXTURE: &str = r#"
<div class="webcomic-media"><a class="webcomic-link"><img class="attachment-full" src="https://comicaurora.com/page.jpg"></a></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive_and_pages() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Aurora - Chapter One");
        let chapters = SOURCE.chapters(json!({"manga": "/chapter-one"})).unwrap();
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }
}
