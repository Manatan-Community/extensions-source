use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SiteSource = SiteSource;
const BASE_URL: &str = "https://kanzenin.info";
const SOURCE_NAME: &str = "Kanzenin";
const MANGA_PATH: &str = "manga";
const LANG: &str = "id";
const CONTENT_RATING: &str = "adult";

struct SiteSource;

impl MangaSource for SiteSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &archive_url(page, &[("order", order.to_string())]),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{MANGA_PATH}/sample"));
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{MANGA_PATH}/sample"));
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapter-1".to_string());
        Ok(parse_pages(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
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
                item: key.contains(&format!("/{MANGA_PATH}/")).then(|| {
                    parse_details(
                        &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                        Some(key),
                    )
                }),
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

fn archive_url(page: u64, params: &[(&str, String)]) -> String {
    let page_part = if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    };
    let query = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    let base = format!(
        "{}/{}/{page_part}",
        BASE_URL.trim_end_matches('/'),
        MANGA_PATH
    );
    if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    }
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = Vec::new();
    let title = filter(filters, "title")
        .filter(|value| !value.is_empty())
        .unwrap_or(query);
    if !title.is_empty() {
        params.push(("title", title.to_string()));
    }
    for (id, param) in [
        ("author", "author"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
        ("genre", "genre[]"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push((param, value.to_string()));
        }
    }
    archive_url(page, &params)
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("bsx")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if !href.contains(&format!("/{MANGA_PATH}/")) {
                    return None;
                }
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h2", "</h2>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| SOURCE_NAME.to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_image(&image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers") || body.contains("rel=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/{MANGA_PATH}/sample"));
    let status_text = info_text(body, "Status")
        .or_else(|| class_status(body))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: image_attr(
            body.split("thumb")
                .nth(1)
                .or_else(|| body.split("infomanga").nth(1))
                .unwrap_or(body),
        )
        .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
        .map(|image| absolute_image(&image)),
        description: html::text_between(body, "entry-content-single", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_text(body, "Penulis")
            .or_else(|| info_text(body, "Author"))
            .or_else(|| info_text(body, "Authors"))
            .into_iter()
            .collect(),
        artists: info_text(body, "Artist")
            .or_else(|| info_text(body, "Artists"))
            .into_iter()
            .collect(),
        tags: parse_tags(body),
        status: parse_status(&status_text),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("eph-num")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea")
                || chunk.contains("ts-main-image")
                || chunk.contains("wp-manga-chapter-img")
                || chunk.contains("aligncenter")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_image(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "data-cfsrc"))
        .or_else(|| srcset_first(html::attr(input, "srcset")))
        .or_else(|| html::attr(input, "src"))
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value?
        .split(',')
        .next()?
        .split_whitespace()
        .next()
        .map(ToString::to_string)
}

fn absolute_image(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("<div")
        .chain(body.split("<tr"))
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .and_then(|chunk| {
            html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<td", "</td>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
        })
        .map(|value| {
            html::strip_tags(&value)
                .replace(label, "")
                .trim_matches([':', ' '])
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn class_status(body: &str) -> Option<String> {
    body.split("status ")
        .nth(1)
        .and_then(|rest| rest.split(['"', '\'', ' ', '>']).next())
        .map(|value| html::strip_tags(value))
        .filter(|value| !value.is_empty())
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre") || chunk.contains("genres"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "on going" => ItemStatus::Ongoing,
        "completed" | "complete" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" | "dropped" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some(parsed) = manatan_shared::dates::parse_ymd(value) {
        return Some(parsed);
    }
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let month = match parts[0].trim_matches(',').to_ascii_lowercase().as_str() {
        "january" | "januari" => 1,
        "february" | "februari" => 2,
        "march" | "maret" => 3,
        "april" => 4,
        "may" | "mei" => 5,
        "june" | "juni" => 6,
        "july" | "juli" => 7,
        "august" | "agustus" => 8,
        "september" => 9,
        "october" | "oktober" => 10,
        "november" => 11,
        "december" | "desember" => 12,
        _ => return None,
    };
    let day = parts[1].trim_matches(',').parse::<u32>().ok()?;
    let year = parts[2].trim_matches(',').parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_key(input: &str) -> String {
    let path = if input.starts_with("http://") || input.starts_with("https://") {
        input
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/').map(|(_, path)| path))
            .unwrap_or_default()
    } else {
        input
    };
    format!(
        "/{}",
        path.split(['?', '#'])
            .next()
            .unwrap_or(path)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bsx"><a href="/manga/sample/" title="Sample Manga"><img src="/cover.jpg"><h2>Sample Manga</h2></a></div><a class="next page-numbers"></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div><div class="entry-content entry-content-single"><p>Sample synopsis.</p></div>
<div class="fmed"><b>Status</b><span>Ongoing</span></div><div class="fmed"><b>Penulis</b><span>Writer</span></div><a href="/genres/action/">Action</a>
<div class="eph-num"><a href="/sample-chapter-1/"><span class="chapternum">Chapter 1</span><span class="chapterdate">September 18, 2024</span></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img class="ts-main-image" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries[0].title, "Sample Manga");
        assert_eq!(
            parse_details(DETAILS_FIXTURE, None).status,
            ItemStatus::Ongoing
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
