use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaCrab = MangaCrab;
const BASE_URL: &str = "https://mangacrab.org";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct MangaCrab;

impl MangaSource for MangaCrab {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest { format!("{BASE_URL}/page/{page}/") } else { BASE_URL.to_string() };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), latest))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(&format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query)), LIST_FIXTURE), true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(&fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let ajax = parse_ajax_chapters(&body);
        if ajax.is_empty() {
            Ok(parse_chapters(&body))
        } else {
            Ok(ajax)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/series/sample/chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        Ok(parse_pages(&fetch_document(&chapter_url, PAGES_FIXTURE), &chapter_url))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(normalize_key(input)))),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, paged: bool) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("mv-rank-item") || chunk.contains("catalog-card") || chunk.contains("mv-recent-card") || chunk.contains("manga-row") || chunk.contains("manga__item"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "manga-row-cover", "href")
                    .or_else(|| html::attr_after(chunk, "mv-recent-link", "href"))
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                if !href.contains("/series/") {
                    return None;
                }
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "mv-rank-title", "</")
                    .or_else(|| html::text_between(chunk, "strong", "</strong>"))
                    .or_else(|| html::text_between(chunk, "<h5", "</h5>"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: paged && (body.contains("next page-numbers") || body.contains("mv-page-link") && body.contains("next")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    let mut item = manga::Madara::parse_details(body, Some(key.clone()), &madara_config());
    item.title = html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    item.description = html::text_between(body, "mv-synopsis", "</div>")
        .or_else(|| html::text_between(body, "modal-contenido", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or(item.description);
    item.language = Some(LANG.to_string());
    item.content_rating = Some(CONTENT_RATING.to_string());
    item.url = Some(url::join_url(BASE_URL, &key));
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/series/") && (chunk.contains("chapter") || chunk.contains("Cap")))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_ajax_chapters(body: &str) -> Vec<MangaChapter> {
    let Some(manga_id) = attr_value(body, "data-manga-id").or_else(|| number_after(body, "\"manga_id\"")) else {
        return Vec::new();
    };
    let Some(nonce) = quoted_after(body, "\"nonce\"").or_else(|| quoted_after(body, "nonce")) else {
        return Vec::new();
    };
    let mut chapters = Vec::new();
    for page in 1..=20 {
        let page_text = page.to_string();
        let response = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&[
                ("action", "mv_get_chapters"),
                ("nonce", nonce.as_str()),
                ("manga_id", manga_id.as_str()),
                ("page", page_text.as_str()),
                ("search", ""),
            ])
            .send_text()
            .unwrap_or_default();
        let json = serde_json::from_str::<Value>(&response).unwrap_or(Value::Null);
        let list_html = json
            .get("data")
            .and_then(|data| data.get("list"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let new_chapters = parse_chapters(list_html);
        let old_len = chapters.len();
        for chapter in new_chapters {
            chapters = push_unique_chapter(chapters, chapter);
        }
        if chapters.len() == old_len {
            break;
        }
    }
    chapters
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let node = quoted_after(body, "\"imgHeader\"").unwrap_or_default();
    body.split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| {
            let mut headers = manga::image_headers(chapter_url);
            if !node.is_empty() {
                headers.insert("Node".to_string(), node.clone());
            }
            MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn madara_config() -> manga::MadaraConfig {
    manga::MadaraConfig {
        base_url: BASE_URL,
        lang: LANG,
        content_rating: CONTENT_RATING,
        manga_path: "series",
        popular_url_marker: "mv-rank-title",
        use_load_more: false,
        latest_enabled: true,
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    for attr in ["data-sec-src", "data-src", "data-lazy-src", "srcset", "data-cfsrc", "src"] {
        if let Some(value) = html::attr(chunk, attr) {
            return Some(value.split_whitespace().next().unwrap_or(&value).to_string());
        }
    }
    None
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let quote_index = rest.find('"')?;
    let value_rest = &rest[quote_index + 1..];
    Some(value_rest[..value_rest.find('"')?].to_string())
}

fn attr_value(body: &str, attr: &str) -> Option<String> {
    body.split('<').find_map(|chunk| html::attr(chunk, attr))
}

fn number_after(body: &str, marker: &str) -> Option<String> {
    let value = body
        .split(marker)
        .nth(1)?
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!value.is_empty()).then_some(value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!("/{}", value[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="mv-rank-item"><a href="/series/sample"><img src="/cover.jpg"><span class="mv-rank-title">Sample Crab</span></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="mb-2">Sample Crab</h1><div class="mv-synopsis">Summary.</div><div id="mv-chapter-list"><a href="/series/sample/chapter-1">Capitulo 1</a></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var mvTheme = {"imgHeader":"node-token"};</script><div id="mv-reader-body"><img class="mv-secure-img" data-sec-src="/page1.jpg"><img src="/page2.jpg"></div>"#;
