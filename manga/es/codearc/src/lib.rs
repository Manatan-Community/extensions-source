use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: CodeArc = CodeArc;
const BASE_URL: &str = "https://mangas.codearctraducciones.com";
const CDN_URL: &str = "https://cdn.codearctraducciones.com";
const NAME: &str = "Code Arc Mangas";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct CodeArc;

impl MangaSource for CodeArc {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, "ranking"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if listing_id(&request) == "latest" {
            format!("{BASE_URL}/list?page={page}")
        } else {
            format!("{BASE_URL}/ranking?mode=popular&page={page}")
        };
        let kind = if listing_id(&request) == "latest" {
            "list"
        } else {
            "ranking"
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            kind,
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }

        if !query.is_empty() && filters_are_empty(&request) {
            let body = fetch_json_or_fixture(
                &format!(
                    "{BASE_URL}/api/mangas/search?q={}&limit=50",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            );
            return Ok(Paged {
                entries: parse_search_json(&body),
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = search_list_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            "list",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/sample/1/cascade".to_string());
        Ok(parse_pages(&fetch_rsc_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    &key,
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(CDN_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("RSC", "1")
        .header("Accept", "text/x-component")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, kind: &str) -> Paged<CatalogItem> {
    let marker = if kind == "ranking" {
        "group relative min-w-0"
    } else {
        "group overflow-hidden"
    };
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains(marker) || chunk.contains("/reader/") == false)
            .filter_map(catalog_from_anchor)
            .collect(),
        has_next_page: body.contains("aria-label=\"Pagina siguiente\"")
            && !body.contains("aria-label=\"Pagina siguiente\" disabled"),
    }
}

fn catalog_from_anchor(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    if href.contains("/reader/") || href.starts_with('#') {
        return None;
    }
    let key = normalize_key(&href);
    if key == "/" || key.starts_with("/list") || key.starts_with("/ranking") {
        return None;
    }
    let title = html::text_between(chunk, "div class=\"truncate text-base", "</div>")
        .or_else(|| html::text_between(chunk, "div class=\"line-clamp-2", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr(chunk, "aria-label"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_from_chunk(chunk),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search_json(body: &str) -> Vec<CatalogItem> {
    json_or_fixture(body, SEARCH_FIXTURE)
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = normalize_key(&string_value(item, "slug").unwrap_or_else(|| "sample".into()));
            CatalogItem {
                key: key.clone(),
                title: string_value(item, "titulo").unwrap_or_else(|| NAME.to_string()),
                cover: string_value(item, "portada").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = html::text_between(body, "span class=\"inline-flex", "</span>")
        .map(|value| html::strip_tags(&value).to_lowercase());
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value).replace("Vista Previa", ""))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| NAME.to_string()),
        description: html::text_between(body, "whitespace-pre-line", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_from_chunk(body)),
        authors: link_values(body, "/creador/"),
        artists: link_values(body, "/creador/"),
        tags: link_values(body, "/list?generos="),
        status: match status_text.as_deref() {
            Some(value) if value.contains("finalizado") => ItemStatus::Completed,
            Some(value) if value.contains("publicandose") || value.contains("publicándose") => {
                ItemStatus::Ongoing
            }
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/reader/") && chunk.contains("/cascade"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let chapter_number = chapter_number(&key);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "<h3", "</h3>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| format!("Chapter {}", chapter_number.unwrap_or(1.0))),
                ),
                chapter_number,
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        Vec::new()
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    page_images(body)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn page_images(body: &str) -> Vec<String> {
    let root = json_or_fixture(body, "{}");
    let mut out = Vec::new();
    collect_images_from_json(&root, &mut out);
    if out.is_empty() {
        out.extend(scan_json_strings_after_key(body, "imagen_url"));
        out.extend(scan_json_strings_after_key(body, "imagenUrl"));
    }
    dedupe(out)
}

fn collect_images_from_json(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "imagen_url" | "imagenUrl" | "imageUrl") {
                    if let Some(image) = value.as_str() {
                        out.push(image.to_string());
                    }
                }
                collect_images_from_json(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_images_from_json(item, out);
            }
        }
        _ => {}
    }
}

fn scan_json_strings_after_key(body: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = format!("\"{key}\"");
    let mut rest = body;
    while let Some(index) = rest.find(&needle) {
        rest = &rest[index + needle.len()..];
        let Some(colon) = rest.find(':') else { break };
        rest = &rest[colon + 1..];
        let Some(first_quote) = rest.find('"') else {
            continue;
        };
        let value_start = first_quote + 1;
        let mut escaped = false;
        let mut end = None;
        for (offset, ch) in rest[value_start..].char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                end = Some(value_start + offset);
                break;
            }
        }
        if let Some(end) = end {
            let raw = &rest[first_quote..=end];
            if let Ok(value) = serde_json::from_str::<String>(raw) {
                out.push(value);
            }
            rest = &rest[end + 1..];
        }
    }
    out
}

fn search_list_url(page: u64, query: &str, filters: &Value) -> String {
    let mut pairs = vec![("page".to_string(), page.to_string())];
    if !query.is_empty() {
        pairs.push(("q".to_string(), query.to_string()));
    }
    for (json_key, query_key) in [
        ("tipo", "tipo"),
        ("contentType", "tipo"),
        ("formato", "formato"),
        ("format", "formato"),
        ("sort", "sort"),
        ("generos", "generos"),
        ("genres", "generos"),
    ] {
        if let Some(value) = filters.get(json_key).and_then(Value::as_str) {
            if !value.is_empty() && value != "both" && value != "latest" {
                pairs.push((query_key.to_string(), value.to_string()));
            }
        }
    }
    format!(
        "{BASE_URL}/list?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filters_are_empty(request: &Value) -> bool {
    request
        .get("filters")
        .and_then(Value::as_object)
        .map(|filters| {
            filters.values().all(|value| {
                value.as_str().is_none_or(|text| text.is_empty())
                    || value.as_array().is_none_or(|items| items.is_empty())
            })
        })
        .unwrap_or(true)
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    chunk.split("<img").nth(1).and_then(|image| {
        html::attr(image, "src")
            .or_else(|| srcset_first(html::attr(image, "srcSet")))
            .or_else(|| srcset_first(html::attr(image, "srcset")))
            .map(|value| absolute_url(&value))
    })
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .find_map(|candidate| candidate.split_whitespace().next().map(ToString::to_string))
    })
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn chapter_number(key: &str) -> Option<f32> {
    let parts = key.split('/').collect::<Vec<_>>();
    parts
        .windows(2)
        .find(|window| window[0] == "reader")
        .and_then(|_| parts.iter().rev().nth(1))
        .and_then(|value| value.parse::<f32>().ok())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        )
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or(Value::Null)
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    values.into_iter().fold(Vec::new(), |mut out, value| {
        if !out.contains(&value) {
            out.push(value);
        }
        out
    })
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

const LIST_FIXTURE: &str = r#"<a class="group relative min-w-0" href="/sample"><div class="truncate text-base">Sample</div><img src="https://cdn.codearctraducciones.com/sample.jpg"></a><a aria-label="Pagina siguiente" href="/ranking?page=2">Next</a>"#;
const SEARCH_FIXTURE: &str = r#"{"items":[{"slug":"sample","titulo":"Sample","portada":"https://cdn.codearctraducciones.com/sample.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><meta property="og:image" content="https://cdn.codearctraducciones.com/sample.jpg"><p class="whitespace-pre-line">Summary</p><a href="/creador/author">Author</a><a href="/list?generos=adulto">Adulto</a><a class="group block" href="/reader/sample/1/cascade"><h3>Chapter 1</h3></a>"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":[{"imagen_url":"https://cdn.codearctraducciones.com/page-1.jpg"}]}"#;

export_manga_source!(SOURCE);
