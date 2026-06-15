use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MenudoFansub = MenudoFansub;
const BASE_ROOT: &str = "https://www.menudo-fansub.com";
const BASE_URL: &str = "https://www.menudo-fansub.com/slide";
const SOURCE_NAME: &str = "Menudo-Fansub";

struct MenudoFansub;

impl MangaSource for MenudoFansub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let path = if latest {
            if page <= 1 {
                "latest/".to_string()
            } else {
                format!("latest/{page}/")
            }
        } else if page <= 1 {
            "directory/".to_string()
        } else {
            format!("directory/{page}/")
        };
        Ok(parse_listing(&fetch_document(
            &slide_url(&path),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_ROOT) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&slide_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut paged = parse_listing(&fetch_document(
            &slide_url(&format!("directory/{page}/")),
            LIST_FIXTURE,
        ));
        if !query.is_empty() {
            let lower = query.to_ascii_lowercase();
            paged
                .entries
                .retain(|item| item.title.to_ascii_lowercase().contains(&lower));
            paged.has_next_page = false;
        }
        Ok(paged)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch_document(&slide_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch_document(
            &slide_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/es/1/1".into());
        Ok(parse_pages(&fetch_document(
            &slide_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| slide_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| slide_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_ROOT) {
            let key = normalize_key(input);
            let item = if key.starts_with("/series/") {
                Some(parse_details(
                    &fetch_document(&slide_url(&key), DETAILS_FIXTURE),
                    Some(key),
                ))
            } else {
                None
            };
            return Ok(Some(UrlResolveResult {
                item,
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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
        .with_cookies_for(BASE_ROOT)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn slide_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn normalize_key(value: &str) -> String {
    let mut path = value.trim();
    if let Some(rest) = path.strip_prefix(BASE_URL) {
        path = rest;
    } else if let Some(rest) = path.strip_prefix(BASE_ROOT) {
        path = rest.strip_prefix("/slide").unwrap_or(rest);
    }
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div class=\"group\"")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "/series/", "href")?;
                let title = html::attr_after(chunk, "/series/", "title")
                    .or_else(|| {
                        html::text_between(chunk, "class=\"title\"", "</div>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: html::attr_after(chunk, "<img", "src")
                        .map(|image| url::join_url(BASE_ROOT, &image)),
                    url: Some(slide_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("prevnext") && body.contains("Next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "tbtitle", "title"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "large comic", "src")
            .or_else(|| html::attr_after(body, "preview", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_ROOT, &image)),
        description: html::text_between(body, "class=\"info\"", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(slide_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div class=\"element\"")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "/read/", "href")?;
            let title = html::attr_after(chunk, "/read/", "title")
                .or_else(|| {
                    html::text_between(chunk, "class=\"title\"", "</div>")
                        .map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let date_text = html::text_between(chunk, "meta_r", "</div>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: parse_dot_date(&date_text),
                scanlators: scanlators(chunk),
                url: Some(slide_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Some(json) = body
        .split("var pages = ")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .map(|value| format!("{value}]"))
    {
        if let Ok(pages) = serde_json::from_str::<Vec<PageDto>>(&json) {
            return pages
                .into_iter()
                .enumerate()
                .map(|(index, page)| image_page(index, &page.url))
                .collect();
        }
    }
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| image.contains("/content/comics/"))
        .enumerate()
        .map(|(index, image)| image_page(index, &image))
        .collect()
}

fn image_page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_ROOT, image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

#[derive(Debug, Deserialize)]
struct PageDto {
    url: String,
}

fn scanlators(chunk: &str) -> Vec<String> {
    chunk
        .split("/team/")
        .skip(1)
        .filter_map(|part| html::attr_after(part, "<a", "title"))
        .collect()
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|window| window[0].to_ascii_lowercase().contains("chapter"))
        .and_then(|window| window[1].trim_matches(':').parse().ok())
}

fn parse_dot_date(value: &str) -> Option<i64> {
    for token in value.split_whitespace() {
        let parts = token.split('.').collect::<Vec<_>>();
        if parts.len() != 3 {
            continue;
        }
        let year = parts[0].parse::<i32>().ok()?;
        let month = parts[1].parse::<i32>().ok()?;
        let day = parts[2].parse::<i32>().ok()?;
        return unix_date(year, month, day);
    }
    None
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="group"><a href="https://www.menudo-fansub.com/slide/series/sample/"><img class="preview" src="/slide/cover.png" /></a><div class="title"><a href="https://www.menudo-fansub.com/slide/series/sample/" title="Sample">Sample</a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="large comic"><img src="/slide/cover.png" /><h1 class="title">Sample</h1></div>
<div class="element"><div class="title"><a href="https://www.menudo-fansub.com/slide/read/sample/es/1/1/" title="Chapter 1">Chapter 1</a></div><div class="meta_r">by <a href="/slide/team/menudo/" title="Menudo-Fansub">Menudo-Fansub</a>, 2024.04.19</div></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>var pages = [{"url":"https:\/\/www.menudo-fansub.com\/slide\/content\/comics\/sample\/0001.png"}];</script>"#;
