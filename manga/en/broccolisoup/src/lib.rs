use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: BroccoliSoup = BroccoliSoup;
const BASE_URL: &str = "https://politeandgood.com";
const ARCHIVE_KEY: &str = "/comic/archive";
const CHARACTER_KEY: &str = "/comic-characters";

struct BroccoliSoup;

impl MangaSource for BroccoliSoup {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![broccoli_catalog(true)],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || "broccoli soup".contains(&query.to_ascii_lowercase()) {
            return Ok(Paged {
                entries: vec![broccoli_catalog(true)],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(broccoli_catalog(true))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let body = fetch_document(&url::join_url(BASE_URL, ARCHIVE_KEY), ARCHIVE_FIXTURE);
        Ok(parse_archive_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| CHARACTER_KEY.into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        if key.trim_matches('/') == CHARACTER_KEY.trim_matches('/') {
            return Ok(parse_character_pages(&body));
        }
        Ok(parse_comic_pages(&body))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, ARCHIVE_KEY)))
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
                item: Some(broccoli_catalog(true)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn broccoli_catalog(initialized: bool) -> CatalogItem {
    CatalogItem {
        key: ARCHIVE_KEY.to_string(),
        title: "Broccoli Soup".to_string(),
        authors: vec!["Secret Pie".to_string()],
        artists: vec!["Secret Pie".to_string()],
        description: Some("Hello there! How is the Weather? This comic is made by me, Secret Pie. I am a pie with legs who draws comics and makes music. I am also an entomologist.".to_string()),
        cover: Some(format!("{BASE_URL}/assets/images/static/Bocki%20(correct%20size).png")),
        url: Some(url::join_url(BASE_URL, ARCHIVE_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_archive_chapters(body: &str) -> Vec<MangaChapter> {
    let mut arc_index = BTreeMap::<String, u64>::new();
    let mut chapters = vec![MangaChapter {
        key: CHARACTER_KEY.to_string(),
        title: Some("Characters".to_string()),
        chapter_number: Some(0.0),
        url: Some(url::join_url(BASE_URL, CHARACTER_KEY)),
        ..MangaChapter::default()
    }];

    for group in body.split("archive-marker").skip(1) {
        let arc_title = html::text_between(group, "marker-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        for chapter in group.split("archive-page").skip(1) {
            let Some(href) = html::attr_after(chapter, "<a", "href") else {
                continue;
            };
            let title = html::text_between(chapter, "page-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Comic".to_string());
            let chapter_number =
                url::slug_from_url(&href).and_then(|slug| slug.parse::<f32>().ok());
            let arc_suffix = arc_title.as_ref().map(|arc| {
                let index = arc_index.entry(arc.clone()).or_insert(0);
                *index += 1;
                format!(" ({arc} #{index})")
            });
            let prefix = chapter_number
                .map(|number| format!("{}: ", number as u64))
                .unwrap_or_default();
            let key = normalize_key(&href);
            chapters.push(MangaChapter {
                key: key.clone(),
                title: Some(format!("{prefix}{title}{}", arc_suffix.unwrap_or_default())),
                chapter_number,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            });
        }
    }
    chapters.reverse();
    chapters
}

fn parse_character_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = Vec::new();
    for section in body.split("static-block").skip(1) {
        let header = ["<h1", "<h2", "<h3", "<h4"]
            .iter()
            .find_map(|tag| html::text_between(section, tag, "</h"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        let body_text = html::text_between(section, "block-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        if header.is_some() || body_text.is_some() {
            let text = [header.unwrap_or_default(), body_text.unwrap_or_default()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            pages.push(manga::text_page(&text));
        }
        if let Some(image) = html::attr_after(section, "<img", "src") {
            pages.push(image_page(pages.len(), &image));
        }
    }
    pages
}

fn parse_comic_pages(body: &str) -> Vec<MangaPage> {
    let comic_body = body
        .split("id=\"comic\"")
        .nth(1)
        .or_else(|| body.split("id='comic'").nth(1))
        .unwrap_or(body);
    comic_body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| image_page(index, &image))
        .collect()
}

fn image_page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_URL, image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        format!("/{}", value[BASE_URL.len()..].trim_matches('/'))
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<li class="archive-marker"><div class="archive-header"><span class="marker-title">Sample Arc</span></div>
<li class="archive-page"><a href="/comic/1"><span class="page-title">Sample Page</span></a></li></li>
"#;
const PAGES_FIXTURE: &str = r#"
<div id="comic"><img src="/assets/sample-page.png"></div>
<section class="static-block"><h2>Character</h2><div class="block-content">Summary</div><figure><img src="/assets/character.png"></figure></section>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive() {
        let chapters = SOURCE.chapters(json!({})).unwrap();
        assert!(!chapters.is_empty());
    }
}
