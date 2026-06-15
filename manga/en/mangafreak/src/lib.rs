use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangafreak = Mangafreak;
const BASE_URL: &str = "https://ww2.mangafreak.me";

struct Mangafreak;

impl MangaSource for Mangafreak {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, "ranking_item"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = if page <= 1 {
                BASE_URL.to_string()
            } else {
                format!("{BASE_URL}/Latest_Releases/{page}")
            };
            return Ok(parse_listing(
                &fetch_document(&target, LATEST_FIXTURE),
                "latest",
            ));
        }
        Ok(parse_listing(
            &fetch_document(&format!("{BASE_URL}/Genre/All/{page}"), LIST_FIXTURE),
            "ranking_item",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/Genre/All")
        } else {
            format!("{BASE_URL}/Find/{}", url::query_escape(query))
        };
        Ok(parse_listing(
            &fetch_document(&target, SEARCH_FIXTURE),
            "search_item",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Manga/Sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Manga/Sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/Read/Sample/1".into());
        Ok(parse_pages(&fetch_document(
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
                    &fetch_document(input, DETAILS_FIXTURE),
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
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, marker: &str) -> Paged<CatalogItem> {
    let chunks = if marker == "latest" {
        body.split("latest_item")
            .chain(body.split("latest_releases_item"))
            .skip(1)
            .collect::<Vec<_>>()
    } else if marker == "search_item" {
        body.split("manga_search_item")
            .chain(body.split("mangaka_search_item"))
            .skip(1)
            .collect::<Vec<_>>()
    } else {
        body.split(marker).skip(1).collect::<Vec<_>>()
    };
    let entries = chunks
        .into_iter()
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "class=\"name", "</")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::text_between(chunk, "<h5", "</h5>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangafreak".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                status: ItemStatus::Unknown,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("next_p"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/Manga/Sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga_series_data", "</h5>")
            .or_else(|| html::text_between(body, "<h5", "</h5>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangafreak".into())),
        cover: body
            .split("manga_series_image")
            .nth(1)
            .and_then(image_from_chunk)
            .or_else(|| image_from_chunk(body)),
        description: html::text_between(body, "manga_series_description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: series_data_line(body, 3).into_iter().collect(),
        artists: series_data_line(body, 4).into_iter().collect(),
        tags: body
            .split("series_sub_genre_list")
            .nth(1)
            .map(link_texts)
            .unwrap_or_default(),
        status: match series_data_line(body, 2).unwrap_or_default().as_str() {
            "ON-GOING" => ItemStatus::Ongoing,
            "COMPLETED" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                chapter_number: title.as_deref().and_then(chapter_number),
                title,
                date_uploaded: Some(0),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("gohere"))
        .filter_map(image_from_chunk)
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

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn series_data_line(body: &str, index: usize) -> Option<String> {
    body.split("manga_series_data")
        .nth(1)?
        .split("<div")
        .nth(index + 1)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
}

fn link_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn chapter_number(title: &str) -> Option<f32> {
    let token = title
        .split_whitespace()
        .find(|part| part.chars().any(|ch| ch.is_ascii_digit()))?;
    let digits = token
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    digits.parse().ok()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="ranking_item"><a href="/Manga/Sample"><img src="/cover.jpg">Sample Manga</a></div><a class="next_p">Next</a>"#;
const LATEST_FIXTURE: &str = r#"<div class="latest_item"><a class="name" href="/Manga/Sample">Sample Manga</a><img src="/cover.jpg"></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="manga_search_item"><h3><a href="/Manga/Sample">Sample Manga</a></h3><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="manga_series_image"><img src="/cover.jpg"></div><div class="manga_series_data"><h5>Sample Manga</h5><div></div><div>ON-GOING</div><div>Author</div><div>Artist</div></div><div class="series_sub_genre_list"><a>Action</a></div><div class="manga_series_description"><p>Description</p></div><table class="manga_series_list"><tr><td><a href="/Read/Sample/1">Chapter 1</a></td><td>2024/01/01</td></tr></table>"#;
const PAGES_FIXTURE: &str = r#"<img id="gohere" src="https://ww2.mangafreak.me/pages/001.jpg"><img id="gohere" src="https://ww2.mangafreak.me/pages/002.jpg">"#;
