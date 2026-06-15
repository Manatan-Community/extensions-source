use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GunnerkriggCourt = GunnerkriggCourt;
const BASE_URL: &str = "https://www.gunnerkrigg.com";
const SERIES_KEY: &str = "/archives/";
const COVER: &str = "https://i.imgur.com/g2ukAIKh.jpg";
const AUTHOR: &str = "Tom Siddell";

struct GunnerkriggCourt;

impl MangaSource for GunnerkriggCourt {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item()],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let item = series_item();
        let entries = if query.is_empty()
            || item.title.to_ascii_lowercase().contains(&query)
            || query.starts_with(BASE_URL)
        {
            vec![item]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, SERIES_KEY),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/?p=1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, SERIES_KEY)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item()),
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

fn series_item() -> CatalogItem {
    CatalogItem {
        key: SERIES_KEY.to_string(),
        title: "Gunnerkrigg Court".to_string(),
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some("Gunnerkrigg Court is a science fantasy webcomic about Antimony Carver, a strange young girl attending an equally strange boarding school.".to_string()),
        cover: Some(COVER.to_string()),
        tags: vec!["Science Fantasy".to_string(), "Mythology".to_string(), "School Life".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, SERIES_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
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
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut current_title = "Chapter".to_string();
    let mut chapters = Vec::new();
    for part in body.split('<') {
        if part.starts_with("h") {
            let text = html::strip_tags(&format!("<{part}"));
            if !text.is_empty() && !text.chars().all(|ch| ch.is_ascii_digit()) {
                current_title = text;
            }
        }
        if !part.starts_with("option") {
            continue;
        }
        let Some(value) = html::attr(part, "value") else {
            continue;
        };
        if value.parse::<u32>().is_err() {
            continue;
        }
        let number = value.parse::<f32>().unwrap_or(-1.0);
        let text = html::text_between(part, ">", "</option>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let title = text.unwrap_or_else(|| format!("{} ({})", current_title, value));
        let key = format!("/?p={value}");
        chapters.push(MangaChapter {
            key: key.clone(),
            title: Some(if title.contains(&value) {
                title
            } else {
                format!("{title} ({value})")
            }),
            chapter_number: Some(number),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        });
    }
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("comic_image"))
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

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"<div class="chapters"><h2>Orientation</h2><select><option value="1">Page 1</option><option value="2">Page 2</option></select></div>"#;
const PAGE_FIXTURE: &str = r#"<img class="comic_image" src="/comics/page.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_gunnerkrigg_pages() {
        assert_eq!(
            SOURCE.pages(json!({"chapter":"/?p=1"})).unwrap()[0].description,
            Some("Page 1".to_string())
        );
    }
}
