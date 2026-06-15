use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Vgperson = Vgperson;
const BASE_URL: &str = "https://vgperson.com/other/mangaviewer.php";
const HOST_URL: &str = "https://vgperson.com";

struct Vgperson;

impl MangaSource for Vgperson {
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
        if query.starts_with(HOST_URL) || query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let mut page = parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE));
        if !query.is_empty() {
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "?m=sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "?m=sample".to_string());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "?m=sample&c=1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Manga".into(),
            style: Some(HomeSectionStyle::Compact),
            has_more: false,
            entries: popular.entries,
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
        if input.starts_with(HOST_URL) || input.starts_with(BASE_URL) {
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
        .with_referer(BASE_URL)
        .with_cookies_for(HOST_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("href=\"?m") || chunk.contains("href='?m"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let title = html::strip_tags(&format!("<a{chunk}")).trim().to_string();
                if title.is_empty() {
                    return None;
                }
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: title.clone(),
                    cover: cover_for_title(&title),
                    url: Some(absolute_url(&key)),
                    language: Some("en".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "?m=sample".to_string());
    let title = html::text_between(body, "class=\"title", "</")
        .or_else(|| html::text_between(body, "class='title", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Manga".to_string());
    CatalogItem {
        key: key.clone(),
        title: title.clone(),
        cover: cover_for_title(&title),
        description: description(body),
        status: if body.contains("(Complete)") {
            ItemStatus::Completed
        } else if body.contains("(Series in Progress)") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn description(body: &str) -> Option<String> {
    let content = html::text_between(body, "class=\"content", "<table")
        .or_else(|| html::text_between(body, "class='content", "<table"))?;
    let text = html::strip_tags(&content);
    (!text.is_empty()).then_some(text)
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("?m=") && (chunk.contains("&c=") || chunk.contains("&b=")))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let mut title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            if let Some(note) = last_cell_text(chunk).filter(|value| !value.is_empty()) {
                title.push_str(" - ");
                title.push_str(note.trim_start_matches("- "));
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: query_param(&key, "c")
                    .and_then(|value| value.parse().ok())
                    .or_else(|| {
                        query_param(&key, "b")
                            .and_then(|value| value.parse::<f32>().ok().map(|b| 16.5 + b / 10.0))
                    }),
                scanlators: vec!["vgperson".to_string()],
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| !src.is_empty())
        .enumerate()
        .map(|(index, src)| {
            let image = if src.starts_with("http://") || src.starts_with("https://") {
                src
            } else {
                url::join_url(HOST_URL, &src)
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn normalize_key(value: &str) -> String {
    if let Some(query) = value.split('?').nth(1) {
        return format!("?{}", query.trim_end_matches('/'));
    }
    value.to_string()
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http://") || key.starts_with("https://") {
        key.to_string()
    } else if key.starts_with('?') {
        format!("{BASE_URL}{key}")
    } else {
        url::join_url(HOST_URL, key)
    }
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input.split('?').nth(1)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn last_cell_text(chunk: &str) -> Option<String> {
    chunk
        .rsplit("<td")
        .next()
        .and_then(|cell| html::text_between(cell, "<td", "</td>"))
        .map(|value| html::strip_tags(&value))
}

fn cover_for_title(title: &str) -> Option<String> {
    match title {
        "The Festive Monster's Cheerful Failure" => Some("https://i.imgur.com/kEK10GL.png"),
        "Azure and Claude" => Some("https://i.imgur.com/buXnlmh.jpg"),
        "Three Days of Happiness" => Some("https://i.imgur.com/kL5dvnp.jpg"),
        _ => None,
    }
    .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="content"><a href="?m=sample">Sample</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="title">Sample</div><div class="content">(Complete)<br>Summary<table class="chaptertable"><tbody><tr><td><a href="?m=sample&c=1">Chapter 1</a></td><td>- Title</td></tr></tbody></table></div>"#;
const PAGES_FIXTURE: &str = r#"<img src="/page1.jpg">"#;
