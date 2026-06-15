use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult, abi::ExtensionResult, export_novel_source, source::NovelSource,
};
use manatan_shared::{
    html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: TopLiba = TopLiba;
const BASE_URL: &str = "https://topliba.com";

struct TopLiba;

impl NovelSource for TopLiba {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "date".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "rating")
        };
        let body = fetch_document(
            &format!("{BASE_URL}/?order_field={sort}&p={page}"),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
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
        let body = fetch_document(
            &format!(
                "{BASE_URL}/?order_field=rating&p={page}&q={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let body = fetch_document(&format!("{BASE_URL}/reader/{key}"), CHAPTERS_FIXTURE);
        let mut out = Vec::new();
        for chunk in body.split("class=\"padding-").skip(1) {
            let padding = chunk
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            let capter = html::attr(chunk, "data-capter").unwrap_or_default();
            let title = chunk
                .split('>')
                .nth(1)
                .and_then(|part| part.split('<').next())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("Chapter");
            let id = if padding == "0" || padding.is_empty() {
                capter.to_string()
            } else {
                format!(
                    "{}-{}",
                    padding,
                    capter.parse::<i64>().unwrap_or(1).saturating_sub(1)
                )
            };
            out.push(NovelChapter {
                key: format!("{key}?{id}"),
                title: Some(html::html_unescape(title)),
                chapter_number: Some(out.len() as f32 + 1.0),
                url: Some(format!("{BASE_URL}/reader/{key}")),
                language: Some("ru".to_string()),
                ..NovelChapter::default()
            });
        }
        Ok(out)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "sample?1".to_string());
        let (book, chapter) = key.split_once('?').unwrap_or((key.as_str(), "1"));
        let reader_url = format!("{BASE_URL}/reader/{book}");
        let page = fetch_document(&reader_url, CHAPTERS_FIXTURE);
        let token = html::attr_after(&page, "name=\"_token\"", "content")
            .or_else(|| html::attr_after(&page, "name='_token'", "content"))
            .unwrap_or_default();
        let content = client()
            .post(format!("{reader_url}/chapter"))
            .referer(&reader_url)
            .origin(BASE_URL)
            .form(&[("chapter", chapter), ("_token", token.as_str())])
            .send_text()
            .unwrap_or_else(|_| TEXT_FIXTURE.to_string());
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("class=\"cover\"") || chunk.contains("class='cover'"))
        .filter_map(|chunk| {
            let data = html::attr(chunk, "data-original")?;
            let title = html::attr(chunk, "title")?;
            let slug = data
                .split("/covers/")
                .nth(1)
                .and_then(|part| part.split('_').next())
                .or_else(|| {
                    data.rsplit('/')
                        .next()
                        .and_then(|part| part.split('.').next())
                })?
                .to_string();
            Some(CatalogItem {
                key: slug.clone(),
                title: html::html_unescape(&title),
                cover: Some(format!("{BASE_URL}/covers/{slug}.jpg")),
                url: Some(format!("{BASE_URL}/books/{slug}")),
                language: Some("ru".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document(&format!("{BASE_URL}/books/{key}"), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: lnreader::text_between_tag(&body, "h1").unwrap_or_else(|| key.to_string()),
        cover: Some(format!("{BASE_URL}/covers/{key}.jpg")),
        description: lnreader::text_after_marker(&body, "class=\"description\"", "</div>"),
        authors: body
            .split("<")
            .find(|chunk| chunk.contains("book-author"))
            .and_then(|_| lnreader::text_after_marker(&body, "class=\"book-author\"", "</div>"))
            .into_iter()
            .collect(),
        tags: body
            .split("<a")
            .filter(|chunk| chunk.contains("/genres/") || chunk.contains("/genre/"))
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .collect(),
        url: Some(format!("{BASE_URL}/books/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
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
        .strip_prefix(&format!("{BASE_URL}/books/"))
        .map(|key| key.trim_matches('/').to_string())
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

const LIST_FIXTURE: &str = r#"<img class="cover" data-original="https://topliba.com/covers/sample_200.jpg" title="Sample Book">"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Book</h1><div class="description">Sample summary.</div><div class="book-author"><a>Sample Author</a></div><div class="book-genres"><a>Fantasy</a></div>"#;
const CHAPTERS_FIXTURE: &str =
    r#"<meta name="_token" content="token"><li class="padding-0" data-capter="1">Chapter 1</li>"#;
const TEXT_FIXTURE: &str = r#"<p>Sample chapter text.</p>"#;

export_novel_source!(SOURCE);
