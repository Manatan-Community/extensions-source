use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Toonkor = Toonkor;
const DEFAULT_BASE_URL: &str = "https://tkor114.com";
const WEBTOONS_PATH: &str = "/%EC%9B%B9%ED%88%B0";
const LATEST_MODIFIER: &str = "?fil=%EC%B5%9C%EC%8B%A0";

struct Toonkor;

impl MangaSource for Toonkor {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, &base));
        }
        let suffix = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{WEBTOONS_PATH}{LATEST_MODIFIER}")
        } else {
            WEBTOONS_PATH.to_string()
        };
        Ok(parse_listing(
            &fetch_document(&format!("{base}{suffix}"), LIST_FIXTURE, &base),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(&base) || query.contains("tkor") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key, &base), DETAILS_FIXTURE, &base),
                    Some(key),
                    &base,
                )],
                has_next_page: false,
            });
        }
        let target = if query.trim().is_empty() {
            format!("{base}{WEBTOONS_PATH}")
        } else {
            format!(
                "{base}/bbs/search.php?sfl=wr_subject%7C%7Cwr_content&stx={}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE, &base),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| WEBTOONS_PATH.to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key, &base), DETAILS_FIXTURE, &base),
            Some(key),
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| WEBTOONS_PATH.to_string());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key, &base), DETAILS_FIXTURE, &base),
            &key,
            &base,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/toon/1".to_string());
        Ok(parse_pages(
            &fetch_document(&absolute_url(&key, &base), PAGES_FIXTURE, &base),
            &base,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key, &base)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key, &base)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if input.starts_with("http") && input.contains("tkor") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE, &base),
                    Some(key),
                    &base,
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| {
            prefs
                .get("baseUrl")
                .or_else(|| prefs.get("overrideBaseUrl"))
        })
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str, base: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn normalize_key(value: &str) -> String {
    if let Some(index) = value.find("://") {
        let path = value[index + 3..]
            .split_once('/')
            .map(|(_, path)| path)
            .unwrap_or("");
        format!("/{}", path.trim_start_matches('/'))
    } else {
        format!("/{}", value.trim_start_matches('/'))
    }
}

fn absolute_url(key: &str, base: &str) -> String {
    url::join_url(base, key)
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("section-item-inner")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "section-item-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "Toonkor".into())
                    }),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|value| absolute_url(&value, base)),
                url: Some(absolute_url(&key, base)),
                language: Some("ko".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| WEBTOONS_PATH.to_string());
    let table = body.split("bt_view1").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(table, "bt_title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Toonkor".to_string()),
        cover: html::attr_after(table, "bt_thumb", "src").map(|value| absolute_url(&value, base)),
        authors: html::text_between(table, "bt_data", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(table, "bt_over", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key, base)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str, base: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("content__title"))
        .filter_map(|chunk| {
            let title_block = chunk.split("content__title").nth(1)?;
            let key = html::attr(title_block, "data-role")?;
            Some(MangaChapter {
                key: key.clone(),
                title: Some(html::strip_tags(title_block)),
                date_uploaded: html::text_between(chunk, "episode__index", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(parse_date),
                url: Some(absolute_url(&key, base)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key, base)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    let encoded = body
        .split("toon_img")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .unwrap_or_default();
    let decoded = STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| body.to_string());
    decoded
        .split("src=\"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').next())
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(image, base),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_date(input: String) -> Option<i64> {
    let parts = input
        .split('-')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    if parts.len() == 3 {
        Some(parts[0] * 10_000 + parts[1] * 100 + parts[2])
    } else {
        None
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="section-item-inner"><div class="section-item-title"><a href="/sample"><h3>Sample Toonkor</h3></a></div><img src="/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<table class="bt_view1"><td class="bt_title">Sample Toonkor</td><td class="bt_label"><span class="bt_data">Author</span></td><td class="bt_over">Description</td><td class="bt_thumb"><img src="/cover.jpg"></td></table>
<table class="web_list"><tr><td class="content__title" data-role="/toon/1">Chapter 1</td><td class="episode__index">2024-01-01</td></tr></table>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>var toon_img = 'PGltZyBzcmM9Ii9wYWdlMS5qcGciPg==';</script>"#;
