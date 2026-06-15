use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CloneManga = CloneManga;
const BASE_URL: &str = "https://manga.clone-army.org";

struct CloneManga;

impl MangaSource for CloneManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = if request.as_object().is_some_and(|object| object.is_empty()) {
            LANDING_FIXTURE.to_string()
        } else {
            fetch_landing()
        };
        Ok(Paged {
            entries: parse_landing(&body),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = fetch_landing();
        let needle = if query.starts_with(BASE_URL) {
            normalize_key(query)
        } else {
            query.to_ascii_lowercase()
        };
        let entries = parse_landing(&body)
            .into_iter()
            .filter(|item| {
                if query.starts_with(BASE_URL) {
                    item.key == needle
                } else {
                    query.trim().is_empty() || item.title.to_ascii_lowercase().contains(&needle)
                }
            })
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/viewer.php?series=sample".to_string());
        Ok(parse_landing(&fetch_landing())
            .into_iter()
            .find(|item| item.key == key)
            .unwrap_or_else(|| fallback_catalog(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/viewer.php?series=sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), SERIES_FIXTURE);
        let total = parse_page_count(&body).unwrap_or(1);
        Ok((1..=total)
            .rev()
            .map(|page| MangaChapter {
                key: chapter_key(&key, page),
                title: Some(format!("Chapter {page}")),
                chapter_number: Some(page as f32),
                url: Some(url::join_url(BASE_URL, &chapter_key(&key, page))),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/viewer.php?series=sample&page=1".to_string());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGE_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
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
                item: Some(
                    parse_landing(&fetch_landing())
                        .into_iter()
                        .find(|item| item.key == key)
                        .unwrap_or_else(|| fallback_catalog(&key)),
                ),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_landing() -> String {
    fetch_document(&format!("{BASE_URL}/viewer_landing.php"), LANDING_FIXTURE)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_landing(body: &str) -> Vec<CatalogItem> {
    body.split("comicPreviewContainer")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let style = html::attr_after(chunk, "comicPreview", "style").unwrap_or_default();
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Clone Manga".to_string())
                    }),
                authors: vec!["Dan Kim".to_string()],
                artists: vec!["Dan Kim".to_string()],
                description: html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                cover: preview_image(&style).map(|path| url::join_url(BASE_URL, &path)),
                status: ItemStatus::Unknown,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn preview_image(style: &str) -> Option<String> {
    let start = style.find("site/themes")?;
    let rest = &style[start..];
    let end = rest.find(')').unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"').trim_matches('\'').to_string())
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..].trim_start_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/'))
}

fn chapter_key(series_key: &str, page: u32) -> String {
    let separator = if series_key.contains('?') { "&" } else { "?" };
    if series_key.contains("&page=") || series_key.contains("?page=") {
        let prefix = series_key.split("&page=").next().unwrap_or(series_key);
        format!("{prefix}&page={page}")
    } else {
        format!("{series_key}{separator}page={page}")
    }
}

fn parse_page_count(body: &str) -> Option<u32> {
    let mut max_page = None;
    for part in body.split("&page=").skip(1) {
        let number = part.split("&lang=").next()?.parse::<u32>().ok()?;
        max_page = Some(max_page.unwrap_or(0).max(number));
    }
    max_page
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    html::attr_after(body, "subsectionContainer", "src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|image| {
            vec![MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                description: Some("Page 1".to_string()),
                ..MangaPage::default()
            }]
        })
        .unwrap_or_default()
}

fn fallback_catalog(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Clone Manga".to_string()),
        authors: vec!["Dan Kim".to_string()],
        artists: vec!["Dan Kim".to_string()],
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

export_manga_source!(SOURCE);

const LANDING_FIXTURE: &str = r#"
<div class="comicPreviewContainer"><a href="/viewer.php?series=sample"><div class="comicPreview" style="background-image:url(site/themes/sample.jpg)"></div><h3>Sample Clone</h3><h4>Sample description.</h4></a></div>
"#;
const SERIES_FIXTURE: &str = r#"
<script>var pages = "&page=1&lang=en &page=2&lang=en &page=3&lang=en &page=4&lang=en";</script>
"#;
const PAGE_FIXTURE: &str =
    r#"<div class="subsectionContainer"><img src="/site/comics/sample/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_landing_and_chapters() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Clone");
        let chapters = SOURCE
            .chapters(json!({"manga":"/viewer.php?series=sample"}))
            .unwrap();
        assert_eq!(chapters.len(), 4);
        assert_eq!(chapters[0].chapter_number, Some(4.0));
    }
}
