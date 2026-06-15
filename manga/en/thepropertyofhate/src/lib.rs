use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    MangaPageImage, PageContent, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: ThePropertyOfHate = ThePropertyOfHate;
const BASE_URL: &str = "https://jolleycomics.com";
const SERIES_KEY: &str = "/";
const CHAPTERS_KEY: &str = "/TPoH/";
const AUTHOR: &str = "Sarah Jolley";

struct ThePropertyOfHate;

impl MangaSource for ThePropertyOfHate {
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
            || AUTHOR.to_ascii_lowercase().contains(&query)
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
            &url::join_url(BASE_URL, CHAPTERS_KEY),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/TPoH/The_Hook/".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("pageUrl").or_else(|| content.get("page_url")))
            .and_then(Value::as_str)
            .or_else(|| request.get("url").and_then(Value::as_str))
            .unwrap_or(BASE_URL);
        let body = fetch_document(page_url, PAGES_FIXTURE);
        let image = html::attr_after(&body, "comic_comic", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .unwrap_or_else(|| page_url.to_string());
        Ok(MangaPageImage {
            url: url::join_url(BASE_URL, &image),
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "series".into(),
            title: "Series".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: vec![series_item()],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(BASE_URL.to_string()))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let chapter = normalize_key(input)
                .filter(|key| key.starts_with("/TPoH/"))
                .map(|key| {
                    json!(MangaChapter {
                        key: key.clone(),
                        title: Some(url::slug_from_url(&key).unwrap_or_else(|| "Chapter".into())),
                        url: Some(url::join_url(BASE_URL, &key)),
                        ..MangaChapter::default()
                    })
                });
            return Ok(Some(UrlResolveResult {
                item: Some(series_item()),
                chapter,
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
        title: "The Property of Hate".to_string(),
        cover: Some(format!("{BASE_URL}/images/Index/tpoh.png")),
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        status: ItemStatus::Unknown,
        url: Some(BASE_URL.to_string()),
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
    let mut chapters = Vec::new();
    let mut added_active_chapter = false;
    let mut chapter_number = 1.0_f32;
    for chunk in body.split("<option").skip(1) {
        if chunk.contains("value=-1") || chunk.contains("value=\"-1\"") {
            continue;
        }
        let Some(value) = html::attr(chunk, "value") else {
            continue;
        };
        let text = html::text_between(chunk, ">", "</option>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "Chapter".into());
        let is_bold = html::attr(chunk, "style")
            .map(|style| style.to_ascii_lowercase().contains("bold"))
            .unwrap_or(false);
        let chapter_key = if is_bold {
            normalize_key(&value).unwrap_or_else(|| normalize_path(&value))
        } else if added_active_chapter {
            continue;
        } else {
            added_active_chapter = true;
            normalize_key(&value)
                .map(|key| {
                    format!(
                        "{}/",
                        key.trim_end_matches('/')
                            .rsplit_once('/')
                            .map(|(parent, _)| parent)
                            .unwrap_or(key.trim_end_matches('/'))
                    )
                })
                .unwrap_or_else(|| normalize_path(&value))
        };
        let chapter_name = if is_bold {
            text.trim().to_string()
        } else {
            text.split(" : Page")
                .next()
                .unwrap_or(&text)
                .trim()
                .to_string()
        };
        chapters.push(MangaChapter {
            key: chapter_key.clone(),
            title: Some(format!("#{} - {}", chapter_number as i32, chapter_name)),
            chapter_number: Some(chapter_number),
            url: Some(url::join_url(BASE_URL, &chapter_key)),
            language: Some("en".into()),
            ..MangaChapter::default()
        });
        chapter_number += 1.0;
    }
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<option")
        .skip(1)
        .filter(|chunk| {
            !chunk.contains("value=-1")
                && !chunk.contains("value=\"-1\"")
                && !html::attr(chunk, "style")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains("bold")
        })
        .filter_map(|chunk| html::attr(chunk, "value"))
        .enumerate()
        .map(|(index, page_url)| MangaPage {
            content: PageContent::Lazy {
                key: format!("page-{}", index + 1),
                url: None,
                page_url: Some(url::join_url(BASE_URL, &page_url)),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(value: &str) -> Option<String> {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return Some(normalize_path(path));
    }
    if value.starts_with("/TPoH/") {
        return Some(normalize_path(value));
    }
    None
}

fn normalize_path(value: &str) -> String {
    format!("/{}", value.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
<select class="jumpbox">
<option value="-1">Jump</option>
<option value="https://jolleycomics.com/TPoH/The_Hook/" style="font-weight:bold">The Hook</option>
<option value="https://jolleycomics.com/TPoH/The_Hook/2">The Hook : Page 2</option>
</select>
"#;
const PAGES_FIXTURE: &str = r#"
<select class="jumpbox"><option value="https://jolleycomics.com/TPoH/The_Hook/1">The Hook : Page 1</option><option value="https://jolleycomics.com/TPoH/The_Hook/2">The Hook : Page 2</option></select>
<div class="comic_comic"><img src="/images/TPoH/sample.png"></div>
"#;
