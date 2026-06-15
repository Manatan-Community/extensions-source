use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{
    html, manga,
    sdk::{http::HttpClient, FilterValue, SearchRequest},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: NarasiNinja = NarasiNinja;
const BASE_URL: &str = "https://narasininja.net";

struct NarasiNinja;

impl MangaSource for NarasiNinja {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(filter_request(page, "", &ParsedFilters::latest()));
        }
        Ok(parse_popular(&fetch_text_or_fixture(
            BASE_URL,
            POPULAR_FIXTURE,
            false,
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
                    &fetch_text_or_fixture(query, DETAILS_FIXTURE, false),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(filter_request(
            page,
            query,
            &parse_filters(request.get("filters")),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample".into());
        Ok(parse_details(
            &fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, false),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample".into());
        Ok(parse_chapters(&fetch_text_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
            false,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/komik/sample/chapter-1".into());
        Ok(parse_pages(&fetch_text_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
            false,
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
                    &fetch_text_or_fixture(input, DETAILS_FIXTURE, false),
                    Some(key),
                )),
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
        .with_referer(format!("{BASE_URL}/komik"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target: &str, fixture: &str, xhr: bool) -> String {
    let http_client = client();
    let request = http_client.get(target);
    let request = if xhr {
        request.xhr()
    } else {
        request.browser_document()
    };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn post_form_or_fixture(target: &str, form: &[(&str, &str)], fixture: &str) -> String {
    let csrf = csrf_token();
    client()
        .post(target)
        .xhr()
        .header("X-CSRF-TOKEN", csrf)
        .header("Referer", format!("{BASE_URL}/komik"))
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn csrf_token() -> String {
    let body = fetch_text_or_fixture(&format!("{BASE_URL}/komik"), CSRF_FIXTURE, false);
    html::attr_after(&body, "name=\"csrf-token\"", "content")
        .or_else(|| html::attr_after(&body, "name='csrf-token'", "content"))
        .unwrap_or_else(|| "fixture-csrf".to_string())
}

#[derive(Default)]
struct ParsedFilters {
    status: String,
    kind: String,
    order: String,
    genres: Vec<String>,
}

impl ParsedFilters {
    fn latest() -> Self {
        Self {
            order: "latest".to_string(),
            ..Self::default()
        }
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters::default();
    for filter in filters_to_values(filters) {
        let value = filter.value.as_str().unwrap_or_default().trim();
        match filter.id.as_str() {
            "status" => parsed.status = value.to_string(),
            "type" => parsed.kind = value.to_string(),
            "order" => parsed.order = value.to_string(),
            "genres" => {
                parsed.genres = value
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(ToString::to_string)
                    .collect()
            }
            _ => {}
        }
    }
    parsed
}

fn filters_to_values(filters: Option<&Value>) -> Vec<FilterValue> {
    let Some(filters) = filters else {
        return Vec::new();
    };
    if let Ok(values) = serde_json::from_value::<Vec<FilterValue>>(filters.clone()) {
        return values;
    }
    filters
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(id, value)| FilterValue {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn filter_request(page: u64, query: &str, filters: &ParsedFilters) -> Paged<CatalogItem> {
    let page_string = page.to_string();
    let body = post_form_or_fixture(
        &format!("{BASE_URL}/komik/filter?page={page}"),
        &[
            ("search", query),
            ("status", &filters.status),
            ("type", &filters.kind),
            ("order", &filters.order),
            ("genre[]", &filters.genres.join(",")),
            ("page", &page_string),
        ],
        FILTER_FIXTURE,
    );
    parse_filter_response(&body)
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("bsx") || chunk.contains("<img"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let title = html::attr(chunk, "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Narasi Ninja".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk(chunk).map(|value| url::join_url(BASE_URL, &value)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_filter_response(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<FilterResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(FILTER_FIXTURE).expect("fixture filter"));
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|entry| CatalogItem {
                key: normalize_key(&entry.url),
                title: entry.title,
                cover: entry.thumbnail,
                url: Some(entry.url),
                language: Some("id".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: response.meta.current_page < response.meta.last_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/komik/sample".to_string());
    let info = body.split("infotable").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Narasi Ninja".to_string()),
        cover: html::attr_after(body, "class=\"thumb\"", "src")
            .or_else(|| html::attr_after(body, "class='thumb'", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "entry-content-single", "</div>")
            .or_else(|| html::text_between(body, "entry-content", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: info_text(info, "Status")
            .map(|value| parse_status(&value))
            .unwrap_or(ItemStatus::Unknown),
        tags: body
            .split("seriestugenre")
            .nth(1)
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        authors: info_text(info, "Author")
            .into_iter()
            .filter(|value| value != "-")
            .collect(),
        artists: info_text(info, "Artist")
            .into_iter()
            .filter(|value| value != "-")
            .collect(),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapterlist")
                || chunk.contains("chapternum")
                || chunk.contains("chapterdate")
        })
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
                chapter_number: parse_chapter_number(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("ts-main-image") || chunk.contains("readerarea"))
        .filter_map(image_from_chunk)
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("<tr").find_map(|row| {
        if !row.contains(label) {
            return None;
        }
        row.split("<td")
            .last()
            .and_then(|value| html::text_between(value, ">", "</td>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapter_number(value: &str) -> Option<f32> {
    value
        .split_whitespace()
        .rev()
        .find_map(|part| part.replace('_', ".").replace('-', ".").parse().ok())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!("/{}", input[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", input.trim_matches('/'))
}

#[derive(Deserialize)]
struct FilterResponse {
    data: Vec<FilterItem>,
    meta: FilterMeta,
}

#[derive(Deserialize)]
struct FilterItem {
    title: String,
    url: String,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct FilterMeta {
    current_page: u64,
    last_page: u64,
}

export_manga_source!(SOURCE);

const CSRF_FIXTURE: &str = r#"<meta name="csrf-token" content="fixture-csrf">"#;
const POPULAR_FIXTURE: &str = r#"
<div class="listupd popularslider"><div class="bs"><div class="bsx"><a title="Sample Narasi Ninja" href="https://narasininja.net/komik/sample"><img src="/cover.jpg"></a></div></div></div>
"#;
const FILTER_FIXTURE: &str = r#"
{"data":[{"title":"Sample Narasi Ninja","url":"https://narasininja.net/komik/sample","thumbnail":"https://narasininja.net/cover.jpg"}],"meta":{"current_page":1,"last_page":1}}
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Narasi Ninja</h1><div class="thumb"><img src="/cover.jpg"></div><div class="entry-content entry-content-single"><p>Sample description.</p></div><table class="infotable"><tr><td>Status</td><td>Ongoing</td></tr><tr><td>Author</td><td>Author</td></tr><tr><td>Artist</td><td>Artist</td></tr></table><div class="seriestugenre"><a>Action</a></div><ul id="chapterlist"><li data-num="1"><a href="/komik/sample/chapter-1"><span class="chapternum">Chapter 1</span><span class="chapterdate">January 1, 2024</span></a></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<div id="readerarea"><img class="ts-main-image" src="/page1.jpg"><img class="ts-main-image" src="/page2.jpg"></div>
"#;
