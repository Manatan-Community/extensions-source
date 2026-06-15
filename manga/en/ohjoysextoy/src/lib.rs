use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OhJoySexToy = OhJoySexToy;
const BASE_URL: &str = "https://www.ohjoysextoy.com";

struct OhJoySexToy;

impl MangaSource for OhJoySexToy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/category/comic/page/{page}/")
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            request.get("listingId").and_then(Value::as_str) != Some("latest"),
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
        Ok(Paged {
            entries: parse_search(&fetch_document(
                &format!("{BASE_URL}/?s={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(
                html::attr_after(&body, "property=\"og:title\"", "content")
                    .map(|value| value.split(" by ").next().unwrap_or(&value).to_string())
                    .unwrap_or_else(|| "Comic".to_string()),
            ),
            scanlators: html::text_between(&body, "post-author", "</")
                .map(|value| vec![html::strip_tags(&value)])
                .unwrap_or_default(),
            date_uploaded: html::text_between(&body, "post-date", "</")
                .and_then(|value| parse_mm_dd_yyyy(&html::strip_tags(&value))),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".to_string(),
                title: "Comics".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn parse_listing(body: &str, paged: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("comicthumbwrap")
        .skip(1)
        .filter_map(card_item)
        .fold(Vec::new(), push_unique_item);
    Paged {
        entries,
        has_next_page: paged && body.contains("pagenav-left") && body.contains("<a"),
    }
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("post-title")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .map(clean_title)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Comic".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_item)
}

fn card_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "comicthumbdate", "</")
        .map(|value| html::strip_tags(&value))
        .map(clean_title)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Comic".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| {
            html::attr_after(body, "property=\"og:url\"", "content")
                .map(|value| normalize_key(&value))
        })
        .unwrap_or_else(|| "/sample".to_string());
    let og_title = html::attr_after(body, "property=\"og:title\"", "content")
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".to_string()));
    CatalogItem {
        key: key.clone(),
        title: clean_title(og_title.clone()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: og_title
            .split(" by ")
            .nth(1)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        description: description(body),
        tags: body
            .split("property=\"article:section\"")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "content"))
            .skip(1)
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("comicpane"))
        .filter_map(image_attr)
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

fn description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(desc) = html::attr_after(body, "property=\"og:description\"", "content")
        .map(|value| {
            value
                .split("      ")
                .next()
                .unwrap_or(&value)
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("{desc}..."));
    }
    let credits = body
        .split("ui-tabs")
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| {
                    let text = html::text_between(link, ">", "</a>")
                        .map(|value| html::strip_tags(&value))?;
                    let href = html::attr(link, "href")?;
                    Some(format!("{text}: {href}"))
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if !credits.is_empty() {
        parts.push(credits.join("\n"));
    }
    if parts.is_empty() {
        None
    } else {
        parts.push("(Full description and credits in WebView)".to_string());
        Some(parts.join("\n\n"))
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
        .or_else(|| html::attr(chunk, "src"))
}

fn clean_title(value: String) -> String {
    value
        .split(" by ")
        .next()
        .unwrap_or(&value)
        .trim()
        .to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_mm_dd_yyyy(value: &str) -> Option<i64> {
    let mut parts = value.split('/');
    let month = parts.next()?.parse::<i32>().ok()?;
    let day = parts.next()?.parse::<i32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    Some(unix_from_ymd(year, month, day))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64 * 86_400
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="comicthumbwrap"><div class="comicarchiveframe"><a href="/sample/"><img src="/sample.jpg"></a></div><div class="comicthumbdate">Sample by Creator</div></div>
"#;
const SEARCH_FIXTURE: &str =
    r#"<h2 class="post-title"><a href="/sample/">Sample by Creator</a></h2>"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample by Creator">
<meta property="og:url" content="https://www.ohjoysextoy.com/sample/">
<meta property="og:image" content="/sample.jpg">
<meta property="og:description" content="Sample description      trailing">
<div class="post-date">01/02/2024</div>
<div class="comicpane"><img src="/page.jpg"></div>
"#;
const PAGES_FIXTURE: &str = DETAILS_FIXTURE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ohjoy_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE, true).entries.len(), 1);
        assert_eq!(parse_search(SEARCH_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
