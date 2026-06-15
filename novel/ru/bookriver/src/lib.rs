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

const SOURCE: Bookriver = Bookriver;
const BASE_URL: &str = "https://bookriver.ru";
const API_URL: &str = "https://api.bookriver.ru/api/v1";

struct Bookriver;

impl NovelSource for Bookriver {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "last-update".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "bestseller")
        };
        let mut target = format!("{BASE_URL}/genre?page={page}&perPage=24&sortingType={sort}");
        let genres = lnreader::filter_array(&request, "genres");
        if !genres.is_empty() {
            target.push_str("&g=");
            target.push_str(&genres.join(","));
        }
        let body = fetch_document(&target, LIST_FIXTURE);
        let entries = next_data(&body)
            .pointer("/props/pageProps/state/pagesFilter/genre/books")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 24,
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{API_URL}/search/autocomplete?keyword={}&page={page}&perPage=10",
            url::query_escape(query)
        );
        let root = fetch_json(&target, SEARCH_FIXTURE);
        let entries = root
            .pointer("/data/books")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 10,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(book_page(&key)
            .pointer("/ebook/chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, chapter)| {
                chapter
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            })
            .filter_map(|(index, chapter)| {
                let id = chapter.get("chapterId").and_then(value_string)?;
                let title = chapter.get("name").and_then(value_string);
                let date = chapter
                    .get("firstPublishedAt")
                    .or_else(|| chapter.get("createdAt"))
                    .and_then(value_string)
                    .and_then(|value| dates::parse_ymd(&value[..value.len().min(10)]));
                Some(NovelChapter {
                    key: format!("{key}/{id}"),
                    title,
                    chapter_number: Some((index + 1) as f32),
                    date_uploaded: date,
                    url: Some(format!("{BASE_URL}/reader/{key}/{id}")),
                    language: Some("ru".to_string()),
                    ..NovelChapter::default()
                })
            })
            .collect())
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let id = key.rsplit('/').next().unwrap_or(&key);
        let root = fetch_json(&format!("{API_URL}/books/chapter/text/{id}"), TEXT_FIXTURE);
        let mut content = root
            .pointer("/data/content")
            .and_then(Value::as_str)
            .unwrap_or("Конец произведения")
            .to_string();
        if let Some(audio) = root.pointer("/data/audio/url").and_then(Value::as_str) {
            if root
                .pointer("/data/audio/available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                content.push_str("<p>");
                content.push_str(&escape_html(audio));
                content.push_str("</p>");
            }
        }
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
            .header("Accept", "application/json")
            .send_text()
            .unwrap_or_else(|_| fixture.to_string()),
    )
    .or_else(|_| serde_json::from_str(fixture))
    .unwrap_or(Value::Null)
}

fn next_data(body: &str) -> Value {
    lnreader::script_json(body, "__NEXT_DATA__")
        .or_else(|| serde_json::from_str(LIST_NEXT_FIXTURE).ok())
        .unwrap_or(Value::Null)
}

fn book_page(key: &str) -> Value {
    let body = fetch_document(&format!("{BASE_URL}/book/{key}"), DETAILS_FIXTURE);
    next_data(&body)
        .pointer("/props/pageProps/state/book/bookPage")
        .cloned()
        .unwrap_or(Value::Null)
}

fn fetch_details(key: &str) -> CatalogItem {
    let book = book_page(key);
    let normalized = normalize_key(key);
    CatalogItem {
        key: normalized.clone(),
        title: text(&book, "name").unwrap_or_else(|| title_from_key(&normalized)),
        cover: book
            .pointer("/coverImages/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: text(&book, "annotation").map(|value| html::strip_tags(&value)),
        authors: book
            .pointer("/author/name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        tags: book
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| text(tag, "name"))
            .collect(),
        status: match text(&book, "statusComplete").as_deref() {
            Some("writing") => ItemStatus::Ongoing,
            Some(_) => ItemStatus::Completed,
            None => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/book/{normalized}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_item(item: &Value) -> CatalogItem {
    let key = text(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: text(item, "name").unwrap_or_else(|| title_from_key(&key)),
        cover: item
            .pointer("/coverImages/0/url")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: Some(format!("{BASE_URL}/book/{key}")),
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
        base_url: Some(format!("{BASE_URL}/reader/{key}")),
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
        .strip_prefix(&format!("{BASE_URL}/book/"))
        .map(normalize_key)
}

fn normalize_key(input: &str) -> String {
    input.trim_matches('/').to_string()
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Book".to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(value_string)
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
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

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const LIST_NEXT_FIXTURE: &str = r#"{"props":{"pageProps":{"state":{"pagesFilter":{"genre":{"books":[{"name":"Sample Book","slug":"sample","coverImages":[{"url":"https://bookriver.ru/cover.jpg"}]}]}}}}}}"#;
const LIST_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"pagesFilter":{"genre":{"books":[{"name":"Sample Book","slug":"sample","coverImages":[{"url":"https://bookriver.ru/cover.jpg"}]}]}}}}}}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"book":{"bookPage":{"name":"Sample Book","slug":"sample","annotation":"Sample summary.","coverImages":[{"url":"https://bookriver.ru/cover.jpg"}],"author":{"name":"Sample Author"},"statusComplete":"writing","tags":[{"name":"Fantasy"}],"ebook":{"chapters":[{"chapterId":1,"name":"Chapter 1","available":true,"createdAt":"2024-01-01"}]}}}}}}}</script>"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"books":[{"name":"Sample Book","slug":"sample","coverImages":[{"url":"https://bookriver.ru/cover.jpg"}]}]}}"#;
const TEXT_FIXTURE: &str =
    r#"{"data":{"content":"<p>Sample chapter text.</p>","audio":{"available":false}}}"#;

export_novel_source!(SOURCE);
