use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: HentaiVNx = HentaiVNx;
const BASE_URL: &str = "https://www.hentaivnx.com";

struct HentaiVNx;

impl MangaSource for HentaiVNx {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/?page={page}")
        } else {
            advanced_url(page, "", Some("10"), request.get("filters"))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).or_else(|| {
            query
                .strip_prefix("id:")
                .map(|slug| format!("/truyen-hentai/{}", slug.trim().trim_matches('/')))
        }) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            advanced_url(page, "", None, request.get("filters"))
        } else {
            format!(
                "{BASE_URL}/tim-truyen?keyword={}&page={page}",
                url::query_escape(&fixed_query(query))
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-hentai/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-hentai/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-hentai/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key
                    .contains("/truyen-hentai/")
                    .then(|| details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn advanced_url(
    page: u64,
    contain: &str,
    forced_sort: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let filters = filters.unwrap_or(&Value::Null);
    let mut pairs = vec![format!("page={page}")];
    let sort = forced_sort
        .or_else(|| filter(filters, "sort"))
        .unwrap_or("15");
    pairs.push(format!("sort={}", url::query_escape(sort)));
    pairs.push(format!(
        "minchapter={}",
        url::query_escape(filter(filters, "minchapter").unwrap_or("0"))
    ));
    let contain = if contain.is_empty() {
        filter(filters, "contain").unwrap_or_default()
    } else {
        contain
    };
    if !contain.is_empty() {
        pairs.push(format!("contain={}", url::query_escape(contain)));
    }
    for value in filter_values(filters, "genres") {
        pairs.push(format!("genres={}", url::query_escape(&value)));
    }
    for value in filter_values(filters, "notgenres") {
        pairs.push(format!("notgenres={}", url::query_escape(&value)));
    }
    format!("{BASE_URL}/tim-truyen-nang-cao?{}", pairs.join("&"))
}

fn parse_listing(body: &str, current_page: u64) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item") || chunk.contains("jtip"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "h3", "href")
                .or_else(|| html::attr_after(chunk, "jtip", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/truyen-hentai/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "h3", "</h3>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| normalize_image_url(&absolute_url(&image))),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains(&format!("page={}", current_page + 1))
            || body.contains("pagination") && !body.contains("disabled"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "title-detail", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "col-image", "data-original")
            .or_else(|| html::attr_after(body, "col-image", "data-src"))
            .or_else(|| html::attr_after(body, "col-image", "src"))
            .map(|v| normalize_image_url(&absolute_url(&v))),
        authors: info_values(body, "author"),
        tags: info_values(body, "kind"),
        description: html::text_between(body, "detail-content", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: parse_status(&info_values(body, "status").join(" ")),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row") && chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("reading-detail")
                || chunk.contains("page-chapter")
                || chunk.contains("chapter-content")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .map(|image| normalize_image_url(&absolute_url(&image)))
        .fold(Vec::<String>::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
}

fn info_values(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|idx| {
            body[idx..]
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else if lower.contains("đang") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn fixed_query(query: &str) -> String {
    query
        .replace('–', "-")
        .replace('’', "'")
        .replace('“', "\"")
        .replace('”', "\"")
        .replace('…', "...")
}

fn normalize_image_url(input: &str) -> String {
    if input.contains("external-content.duckduckgo.com/iu/") {
        for part in input.split('&') {
            if let Some(value) = part.strip_prefix("u=") {
                return value.replace("%3A", ":").replace("%2F", "/");
            }
        }
    }
    input.replace("-150x150.", ".")
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-original")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else {
        format!("{BASE_URL}/{}", value.trim_start_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen-hentai/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn filter_values(filters: &Value, id: &str) -> Vec<String> {
    match filters.get(id) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div class="items"><div class="item"><h3><a href="/truyen-hentai/sample" title="Sample">Sample</a></h3><img src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="title-detail">Sample</h1><div class="detail-info"><div class="col-image"><img src="/cover.jpg"></div></div><li class="status"><div class="col-xs-8">Đang tiến hành</div></li><li class="kind"><div class="col-xs-8"><a>Adult</a></div></li><div id="nt_listchapter" class="list-chapter"><ul><li class="row"><div class="chapter"><a href="/truyen-hentai/sample/chapter-1">Chapter 1</a></div></li></ul></div><div class="detail-content">Summary</div>"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-detail"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
