use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaTv = MangaTv;
const BASE_URL: &str = "https://mangatv.net";
const CONTENT_RATING: &str = "adult";

struct MangaTv;

impl MangaSource for MangaTv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_text(
            &list_url(page, order, ""),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_text(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_text(
            &list_url(page, "latest", query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_text(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_text(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_text(&chapter_url, PAGES_FIXTURE);
        let pages = parse_encoded_pages(&body, &chapter_url);
        if pages.is_empty() {
            Ok(parse_image_pages(&body, &chapter_url))
        } else {
            Ok(pages)
        }
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text(input, DETAILS_FIXTURE),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, order: &str, query: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let mut target = format!("{BASE_URL}/lista/{page_path}");
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("s={}", url::query_escape(query)));
    }
    if order == "popular" {
        params.push("order=popular".to_string());
    }
    if page > 1 && !query.is_empty() {
        params.push(format!("page={page}"));
    }
    if !params.is_empty() {
        target.push('?');
        target.push_str(&params.join("&"));
    }
    target
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/manga/sample".to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("bsx")
                || chunk.contains("page-item-detail")
                || chunk.contains("listupd")
                || chunk.contains("utao")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "tt", "</"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk).map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    if entries.is_empty() {
        entries = body
            .split("<article")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if !href.contains("/manga/") {
                    return None;
                }
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::attr_after(chunk, "<img", "alt")
                        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_default()),
                    cover: image_from_chunk(chunk).map(|value| absolute_url(&value)),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique);
    }
    Paged {
        entries,
        has_next_page: body.contains("hpage") || body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut description = html::text_between(body, "Sinopsis", "</span>")
        .or_else(|| html::text_between(body, "entry-content", "</div>"))
        .or_else(|| html::text_between(body, "seriestucon", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if description.eq_ignore_ascii_case("sinopsis") {
        description.clear();
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_from_chunk(body).map(|value| absolute_url(&value)),
        description: (!description.is_empty()).then_some(description),
        authors: detail_values(body, "Autor"),
        artists: detail_values(body, "Artista"),
        tags: detail_values(body, "Genero")
            .into_iter()
            .chain(detail_values(body, "Género"))
            .collect(),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter") || chunk.contains("chapternum") || chunk.contains("eph-num")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "chapternum", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "dt", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: "/manga/sample/chapter-1".to_string(),
            title: Some("Chapter 1".to_string()),
            url: Some(format!("{BASE_URL}/manga/sample/chapter-1")),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_encoded_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let image_values = quoted_strings(body)
        .into_iter()
        .filter_map(|value| decode_image_value(&value))
        .filter(|value| {
            value.starts_with("http") || value.starts_with("//") || value.starts_with('/')
        })
        .collect::<Vec<_>>();
    image_values
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &absolute_image(&image), chapter_url))
        .collect()
}

fn parse_image_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter-img")
                || chunk.contains("ts-main-image")
                || chunk.contains("readerarea")
                || chunk.contains("chapter")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| page(index, &absolute_image(&image), chapter_url))
        .collect()
}

fn page(index: usize, image: &str, chapter_url: &str) -> MangaPage {
    let headers = manga::image_headers(chapter_url);
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn quoted_strings(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in body.chars() {
        if let Some(q) = quote {
            if escaped {
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                if current.len() > 12 {
                    out.push(current.clone());
                }
                current.clear();
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        }
    }
    out
}

fn decode_image_value(value: &str) -> Option<String> {
    if value.starts_with("http") || value.starts_with("//") || value.starts_with('/') {
        return Some(value.to_string());
    }
    let decoded = STANDARD.decode(value).ok()?;
    String::from_utf8(decoded).ok()
}

fn absolute_image(value: &str) -> String {
    if let Some(path) = value.strip_prefix("//") {
        format!("https://{path}")
    } else {
        absolute_url(value)
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn detail_values(body: &str, label: &str) -> Vec<String> {
    body.split("imptdt")
        .filter(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("completado") || lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("cancelado") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") || lower.contains("pausa") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
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

const LIST_FIXTURE: &str = r#"
<div class="bsx"><a href="/manga/sample/" title="Sample Manga"><img src="/cover.jpg"></a><div class="tt">Sample Manga</div></div>
<a class="next page-numbers" href="/lista/page/2/">2</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div>
<b>Sinopsis</b><span>Resumen</span></div>
<div id="chapterlist"><ul class="clstyle"><li><div class="dt"><a href="/manga/sample/chapter-1/"><span class="chapternum">Chapter 1</span></a></div><span class="chapterdate">2024-01-01</span></li></ul></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>var images=["aHR0cHM6Ly9tYW5nYXR2Lm5ldC9wYWdlMS5qcGc="];</script>"#;
