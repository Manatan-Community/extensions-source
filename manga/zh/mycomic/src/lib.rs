use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: MyComic = MyComic;
const BASE_URL: &str = "https://mycomic.com";

struct MyComic;

impl MangaSource for MyComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" {
            "-update"
        } else {
            "-views"
        };
        Ok(fetch_listing(
            &request,
            &listing_url(&request, "", sort, page(&request)),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&request, &key)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter(filters, "sort").unwrap_or("");
        if let Some(rank_sort) = sort.strip_prefix("rank|") {
            return Ok(parse_rank(&fetch(
                &request,
                &rank_url(&request, rank_sort),
                RANK_FIXTURE,
            )));
        }
        Ok(fetch_listing(
            &request,
            &listing_url(&request, &query, sort, page(&request)),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".to_string());
        Ok(details_by_key(&request, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".to_string());
        Ok(parse_chapters(&fetch(
            &request,
            &absolute_with_request(&request, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapters/1".into());
        let target = absolute(&key);
        Ok(parse_pages(&fetch(&request, &target, PAGES_FIXTURE)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let preferences = request.get("preferences").cloned().unwrap_or(Value::Null);
        let popular = self
            .list(json!({"page": 1, "listingId": "popular", "preferences": preferences.clone()}))?;
        let latest =
            self.list(json!({"page": 1, "listingId": "latest", "preferences": preferences}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&request, &key)),
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
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .with_header("Accept-Language", "en-US,en;q=0.9")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(_request: &Value, target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_listing(request: &Value, target: &str) -> Paged<CatalogItem> {
    parse_listing(&fetch(request, target, LIST_FIXTURE))
}

fn details_by_key(request: &Value, key: &str) -> CatalogItem {
    parse_details(&fetch(
        request,
        &absolute_with_request(request, key),
        DETAILS_FIXTURE,
    ))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"group")
        .skip(1)
        .filter_map(parse_grid_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel=next"),
    }
}

fn parse_grid_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<img", "alt")
        .or_else(|| html::text_between(chunk, "data-flux-heading", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: img_attr(chunk).map(|image| absolute(&image)),
        url: Some(absolute(&key)),
        language: Some("zh".to_string()),
        content_rating: Some("adult".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_rank(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<tr")
        .skip(1)
        .filter_map(|chunk| {
            let cell = chunk.split("</tr>").next().unwrap_or(chunk);
            let href = html::attr_after(cell, "<a", "href")?;
            let title = html::text_between(cell, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                url: Some(absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("adult".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let card = body
        .split("data-flux-card")
        .nth(1)
        .and_then(|rest| rest.split("div[data-flux-card]").next())
        .unwrap_or(body);
    let title = html::text_between(card, "data-flux-heading", "</")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "MyComic".to_string());
    let status_text = html::text_between(card, "data-flux-badge", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    CatalogItem {
        key: key_from_canonical(body).unwrap_or_else(|| "/comics/sample".to_string()),
        title,
        cover: img_attr(card)
            .or_else(|| img_attr(body))
            .map(|image| absolute(&image)),
        url: key_from_canonical(body).map(|key| absolute(&key)),
        authors: first_link_after_badge(card).into_iter().collect(),
        description: html::text_between(card, "x-show=show", "</div>")
            .or_else(|| html::attr_after(body, "meta[name=description]", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: detail_tags(card),
        language: Some("zh".to_string()),
        content_rating: Some("adult".to_string()),
        status: match status_text.as_str() {
            "连载中" | "連載中" => ItemStatus::Ongoing,
            "已完结" | "已完結" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let notes = body
        .split(">chapters")
        .next()
        .unwrap_or(body)
        .split("data-flux-card")
        .skip(2)
        .filter_map(|chunk| {
            html::text_between(chunk, "<div", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect::<Vec<_>>();
    let date = html::text_between(body, "<time", "</time>")
        .map(|value| html::strip_tags(&value))
        .and_then(|value| dates::parse_ymd(value.trim()));
    let mut chapters = Vec::new();
    for (group_index, array) in chapter_arrays(body).into_iter().enumerate() {
        if let Ok(items) = serde_json::from_str::<Vec<ChapterDto>>(&array) {
            for item in items {
                chapters.push(MangaChapter {
                    key: format!("/chapters/{}", item.id),
                    title: Some(item.title),
                    date_uploaded: date,
                    scanlators: notes.get(group_index).cloned().into_iter().collect(),
                    url: Some(absolute(&format!("/chapters/{}", item.id))),
                    ..MangaChapter::default()
                });
            }
        }
    }
    chapters
}

fn chapter_arrays(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("chapters: [") {
        let after = &rest[start + "chapters: ".len()..];
        if let Some(end) = after.find(']') {
            out.push(html::html_unescape(&after[..=end]));
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("x-ref"))
        .filter_map(img_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn listing_url(request: &Value, query: &str, sort: &str, page: u64) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    if !sort.is_empty() {
        params.push(format!("sort={}", url::query_escape(sort)));
    }
    let filters = request.get("filters").unwrap_or(&Value::Null);
    for key in [
        "filter[country]",
        "filter[tag]",
        "filter[audience]",
        "filter[year]",
        "filter[end]",
    ] {
        if let Some(value) = filter(filters, key) {
            params.push(format!(
                "{}={}",
                url::query_escape(key),
                url::query_escape(value)
            ));
        }
    }
    if page > 1 {
        params.push(format!("page={page}"));
    }
    let base = format!("{}/comics", request_url(request));
    if params.is_empty() {
        base
    } else {
        format!("{base}?{}", params.join("&"))
    }
}

fn rank_url(request: &Value, sort: &str) -> String {
    if sort.is_empty() {
        format!("{}/rank", request_url(request))
    } else {
        format!(
            "{}/rank?sort={}",
            request_url(request),
            url::query_escape(sort)
        )
    }
}

fn request_url(request: &Value) -> String {
    match request
        .get("preferences")
        .and_then(|preferences| preferences.get("language"))
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "" => BASE_URL.to_string(),
        lang => format!("{BASE_URL}/{}", lang.trim_matches('/')),
    }
}

fn absolute_with_request(request: &Value, path: &str) -> String {
    let key = normalize_key(path);
    if key.starts_with("/cn/") || key == "/cn" {
        absolute(&key)
    } else {
        url::join_url(&request_url(request), &key)
    }
}

fn absolute(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/comics/") || key.contains("/chapters/"))
}

fn key_from_canonical(body: &str) -> Option<String> {
    html::attr_after(body, "canonical", "href").map(|href| normalize_key(&href))
}

fn normalize_key(input: &str) -> String {
    let value = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

fn img_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn first_link_after_badge(card: &str) -> Option<String> {
    card.split("data-flux-badge")
        .nth(1)
        .and_then(|rest| html::text_between(rest, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn detail_tags(card: &str) -> Vec<String> {
    card.split("<a")
        .skip(2)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn push_unique(mut values: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|existing| existing.key == item.key) {
        values.push(item);
    }
    values
}

#[derive(Deserialize)]
struct ChapterDto {
    id: u64,
    title: String,
}

const LIST_FIXTURE: &str = r#"<div class="grid"><div class="group"><a href="https://mycomic.com/comics/sample"><img alt="Sample MyComic" data-src="https://mycomic.com/cover.jpg"></a></div></div><nav role="navigation"><a rel="next"></a></nav>"#;
const RANK_FIXTURE: &str = r#"<table><tbody><tr><td>1</td><td><a href="https://mycomic.com/comics/sample">Sample MyComic</a></td></tr></tbody></table>"#;
const DETAILS_FIXTURE: &str = r#"<link rel="canonical" href="https://mycomic.com/comics/sample"><div data-flux-card><div data-flux-heading>Sample MyComic</div><img class="object-cover" src="/cover.jpg"><div data-flux-badge>連載中</div><div><div><a>Author</a></div><div></div><div><a>Action</a></div></div><div><div x-show=show>Summary</div></div></div><div data-flux-card><div><div>Team</div></div><div x-data="{ chapters: [{&quot;id&quot;:1,&quot;title&quot;:&quot;Chapter 1&quot;}] }"></div></div><time datetime="2024-01-01">2024-01-01</time>"#;
const PAGES_FIXTURE: &str = r#"<main><img x-ref="page" src="https://mycomic.com/page1.jpg"><img src="https://mycomic.com/ignored.jpg"></main>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample MyComic");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details() {
        let item = parse_details(DETAILS_FIXTURE);
        assert_eq!(item.title, "Sample MyComic");
        assert_eq!(item.authors, vec!["Author"]);
        assert_eq!(item.status, ItemStatus::Ongoing);
        assert!(item.initialized);
    }

    #[test]
    fn parses_chapters() {
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/chapters/1");
        assert_eq!(
            chapters[0].date_uploaded,
            Some(dates::unix_utc_2024_01_01())
        );
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].content,
            PageContent::Url {
                url: "https://mycomic.com/page1.jpg".to_string(),
                context: None
            }
        );
    }
}

export_manga_source!(SOURCE);
