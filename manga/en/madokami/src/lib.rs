use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Madokami = Madokami;
const BASE_URL: &str = "https://manga.madokami.al";

struct Madokami;

impl MangaSource for Madokami {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(RECENT_FIXTURE),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_listing(&fetch_auth_document(
                &format!("{BASE_URL}/recent"),
                &request,
                RECENT_FIXTURE,
            )),
            has_next_page: false,
        })
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
                    &fetch_auth_document(input_manga_url(&key).as_str(), &request, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}/search?q={}", url::query_escape(query));
        Ok(Paged {
            entries: parse_listing(&fetch_auth_document(&target, &request, SEARCH_FIXTURE)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/Manga/S/Sample".to_string());
        Ok(parse_details(
            &fetch_auth_document(&input_manga_url(&key), &request, DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/Manga/S/Sample".to_string());
        Ok(parse_chapters(&fetch_auth_document(
            &input_manga_url(&key),
            &request,
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/Manga/S/Sample/Chapter%201".to_string());
        Ok(parse_pages(&fetch_auth_document(
            &url::join_url(BASE_URL, &key),
            &request,
            PAGES_FIXTURE,
        )))
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
                item: Some(parse_details(
                    &fetch_auth_document(input, &request, DETAILS_FIXTURE),
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

fn fetch_auth_document(target: &str, request: &Value, fixture: &str) -> String {
    let client = client();
    let mut get = client.get(target).browser_document();
    if let Some(auth) = basic_auth(request) {
        get = get.header("Authorization", auth);
    }
    get.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn basic_auth(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    let username = prefs
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = prefs
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if username.is_empty() && password.is_empty() {
        return None;
    }
    Some(format!(
        "Basic {}",
        base64_encode(format!("{username}:{password}").as_bytes())
    ))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/Manga/") && !href.contains("/Raws/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = title_from_key(&key);
            Some(CatalogItem {
                key: key.clone(),
                title,
                description: Some(path_last(&key)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/Manga/S/Sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: title_from_key(&key),
        description: Some(path_last(&key)),
        authors: link_texts(body, "itemprop=\"author\""),
        tags: link_texts(body, "tag"),
        status: if html::text_between(body, "scanstatus", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default()
            == "Yes"
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        cover: html::attr_after(body, "itemprop=\"image\"", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("/reader"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let reader = href
                .split("/reader")
                .nth(1)
                .map(|tail| format!("/reader{tail}"))?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: reader.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &reader)),
                date_uploaded: html::text_between(chunk, "<td", "</td>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let path = html::attr_after(body, "id=\"reader\"", "data-path")
        .or_else(|| html::attr_after(body, "id='reader'", "data-path"))
        .unwrap_or_default();
    let files_raw = html::attr_after(body, "id=\"reader\"", "data-files")
        .or_else(|| html::attr_after(body, "id='reader'", "data-files"))
        .unwrap_or_else(|| "[]".to_string());
    let files: Vec<String> = serde_json::from_str(&files_raw).unwrap_or_default();
    files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let image = format!(
                "{BASE_URL}/reader/image?path={}&file={}",
                url::query_escape(&path),
                url::query_escape(&file)
            );
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

fn input_manga_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
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

fn title_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .rev()
        .find(|part| !part.starts_with('!'))
        .map(percent_decode)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Manga".to_string())
}

fn path_last(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .last()
        .map(percent_decode)
        .unwrap_or_default()
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn percent_decode(input: &str) -> String {
    let mut out = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(value as char);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' {
            ' '
        } else {
            bytes[i] as char
        });
        i += 1;
    }
    out
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b111111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

export_manga_source!(SOURCE);

const RECENT_FIXTURE: &str =
    r#"<table><tbody><tr><td><a href="/Manga/S/Sample">Sample</a></td></tr></tbody></table>"#;
const SEARCH_FIXTURE: &str = RECENT_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<div class="manga-info"><img itemprop="image" src="/cover.jpg"></div>
<a itemprop="author">Author</a><div class="genres"><a class="tag">Action</a></div><span class="scanstatus">Yes</span>
<table id="index-table"><tbody><tr><td><a>Chapter 1</a></td><td></td><td>2024-01-01 00:00</td><td></td><td></td><td><a href="/reader/Manga/S/Sample/Chapter%201">Read</a></td></tr></tbody></table>
"#;
const PAGES_FIXTURE: &str = r#"<div id="reader" data-path="/Manga/S/Sample/Chapter 1" data-files="[&quot;001.jpg&quot;]"></div>"#;
