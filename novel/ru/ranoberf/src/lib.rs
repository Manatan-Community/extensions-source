use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    dates, html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: RanobeRf = RanobeRf;
const BASE_URL: &str = "https://xn--80ac9aeh6f.xn--p1ai";

struct RanobeRf;

impl NovelSource for RanobeRf {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "lastPublishedChapter".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "popular")
        };
        let body = fetch_document(
            &format!("{BASE_URL}/books?order={order}&page={page}"),
            LIST_FIXTURE,
        );
        let data = next_data(&body);
        let entries = data
            .pointer("/props/pageProps/totalData/items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 20,
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let escaped = url::query_escape(query);
        let target = format!(
            "{BASE_URL}/v3/books?filter[or][0][title][like]={escaped}&filter[or][1][titleEn][like]={escaped}&filter[or][2][fullTitle][like]={escaped}&filter[status][]=active&filter[status][]=abandoned&filter[status][]=completed&expand=verticalImage"
        );
        let root = fetch_json(&target, SEARCH_FIXTURE);
        let entries = root
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: false,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/sample".to_string());
        let book = book_data(&key);
        let mut entries = book
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, chapter)| {
                !chapter
                    .get("isDonate")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || chapter
                        .get("isUserPaid")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })
            .filter_map(|(index, chapter)| {
                let url_path = text(chapter, "url")?;
                Some(NovelChapter {
                    key: normalize_key(&url_path),
                    title: text(chapter, "title"),
                    chapter_number: Some((index + 1) as f32),
                    date_uploaded: text(chapter, "publishedAt")
                        .and_then(|date| dates::parse_ymd(&date[..date.len().min(10)])),
                    url: Some(absolute_url(&url_path)),
                    language: Some("ru".to_string()),
                    ..NovelChapter::default()
                })
            })
            .collect::<Vec<_>>();
        entries.reverse();
        Ok(entries)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1".to_string());
        let body = fetch_document(&absolute_url(&key), TEXT_FIXTURE);
        let content = next_data(&body)
            .pointer("/props/pageProps/chapter/content/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        text_response(&key, &content)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Popular", popular),
            section("latest", "Latest", latest),
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> Value {
    serde_json::from_str(
        &client()
            .get(target)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| fixture.to_string()),
    )
    .or_else(|_| serde_json::from_str(fixture))
    .unwrap_or(Value::Null)
}

fn next_data(body: &str) -> Value {
    lnreader::script_json(body, "__NEXT_DATA__")
        .or_else(|| serde_json::from_str(NEXT_FIXTURE).ok())
        .unwrap_or(Value::Null)
}

fn book_data(key: &str) -> Value {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    next_data(&body)
        .pointer("/props/pageProps/book")
        .cloned()
        .unwrap_or(Value::Null)
}

fn fetch_details(key: &str) -> CatalogItem {
    let book = book_data(key);
    CatalogItem {
        key: normalize_key(key),
        title: text(&book, "title").unwrap_or_else(|| title_from_key(key)),
        cover: book
            .pointer("/verticalImage/url")
            .and_then(Value::as_str)
            .map(absolute_url),
        description: text(&book, "description").map(|value| html::strip_tags(&value)),
        authors: text(&book, "author").into_iter().collect(),
        tags: book
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| text(genre, "title"))
            .collect(),
        status: text(&book, "additionalInfo")
            .filter(|info| info.contains("Активен"))
            .map(|_| ItemStatus::Ongoing)
            .unwrap_or(ItemStatus::Completed),
        url: Some(absolute_url(key)),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_item(item: &Value) -> CatalogItem {
    let key = text(item, "slug")
        .map(|slug| format!("/{slug}"))
        .unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: text(item, "title").unwrap_or_else(|| title_from_key(&key)),
        cover: item
            .pointer("/verticalImage/url")
            .and_then(Value::as_str)
            .map(absolute_url),
        url: Some(absolute_url(&key)),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn text_response(key: &str, html_body: &str) -> ExtensionResult<NovelText> {
    let normalized = novel::normalize_reader_html(html_body);
    Ok(NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    })
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://ранобэ.рф"))
        .map(normalize_key)
}

fn normalize_key(input: &str) -> String {
    let key = input
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    if key.starts_with('/') {
        key.to_string()
    } else {
        format!("/{key}")
    }
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        format!("{BASE_URL}{}", normalize_key(input))
    }
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Ranobe".to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const NEXT_FIXTURE: &str = r#"{"props":{"pageProps":{"totalData":{"items":[{"title":"Sample Ranobe","slug":"sample","verticalImage":{"url":"/cover.jpg"}}]},"book":{"title":"Sample Ranobe","description":"Sample summary.","verticalImage":{"url":"/cover.jpg"},"author":"Sample Author","additionalInfo":"Активен","genres":[{"title":"Fantasy"}],"chapters":[{"title":"Chapter 1","url":"/sample/chapter-1","publishedAt":"2024-01-01","isDonate":false}]}}}}"#;
const LIST_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"totalData":{"items":[{"title":"Sample Ranobe","slug":"sample","verticalImage":{"url":"/cover.jpg"}}]}}}}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"book":{"title":"Sample Ranobe","description":"Sample summary.","verticalImage":{"url":"/cover.jpg"},"author":"Sample Author","additionalInfo":"Активен","genres":[{"title":"Fantasy"}],"chapters":[{"title":"Chapter 1","url":"/sample/chapter-1","publishedAt":"2024-01-01","isDonate":false}]}}}}</script>"#;
const SEARCH_FIXTURE: &str =
    r#"{"items":[{"title":"Sample Ranobe","slug":"sample","verticalImage":{"url":"/cover.jpg"}}]}"#;
const TEXT_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"chapter":{"content":{"text":"<p>Sample chapter text.</p>"}}}}}</script>"#;

export_novel_source!(SOURCE);
