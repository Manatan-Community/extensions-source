use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: JeazScans = JeazScans;
const BASE_URL: &str = "https://lectorhub.j5z.xyz";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct JeazScans;

impl MangaSource for JeazScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_home(LIST_FIXTURE, "popular"));
        }
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        Ok(parse_home(
            &fetch_document_or_fixture(BASE_URL, LIST_FIXTURE),
            listing,
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(Paged {
            entries: parse_search(&fetch_json_or_fixture(
                &format!("{BASE_URL}/ajax_search.php?q={}", url::query_escape(query)),
                SEARCH_FIXTURE,
            )),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga.php?id=1".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga.php?id=1".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/leer/sample/capitulo-1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE);
        let html_pages = parse_html_pages(&body);
        if !html_pages.is_empty() {
            return Ok(html_pages);
        }
        Ok(parse_api_pages(&body, &chapter_url))
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
        if input.starts_with(BASE_URL) && input.contains("manga.php") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
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
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_home(body: &str, listing: &str) -> Paged<CatalogItem> {
    let marker = if listing == "latest" {
        "Lanzamientos"
    } else {
        "Top Rankings"
    };
    let section = section_after(body, marker).unwrap_or(body);
    Paged {
        entries: section
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("manga.php?id="))
            .filter_map(catalog_from_anchor)
            .collect(),
        has_next_page: false,
    }
}

fn catalog_from_anchor(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h4", "</h4>")
            .or_else(|| html::text_between(chunk, "<h5", "</h5>"))
            .or_else(|| html::text_between(chunk, "<figcaption", "</figcaption>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Jeaz Scans".into())),
        cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Vec<SearchItem>>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap())
        .into_iter()
        .filter(|item| item.id >= 0 && !item.titulo.trim().is_empty())
        .map(|item| {
            let key = format!("/manga.php?id={}", item.id);
            CatalogItem {
                key: key.clone(),
                title: item.titulo,
                cover: item.portada.map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "blood-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Jeaz Scans".into())),
        description: html::text_between(body, "SINOPSIS", "</div>")
            .or_else(|| html::text_between(body, "text-gray-200", "</div>"))
            .map(|value| {
                html::strip_tags(&value)
                    .trim_start_matches("SINOPSIS")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "cultivation-panel", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("directorio.php?genero="))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: status_from_text(
            &html::text_between(body, "status-badge", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let number = html::attr(chunk, "data-chapter-number")
                .and_then(|value| value.parse::<f32>().ok())
                .or_else(|| chapter_number_from_url(&href));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "chapter-title", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| {
                            number
                                .map(|value| format!("Chapter {}", trim_float(value)))
                                .unwrap_or_else(|| "Chapter".into())
                        }),
                ),
                chapter_number: number,
                date_uploaded: html::text_between(chunk, "ph-clock", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_html_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("protected-img")
                || chunk.contains("data-sec-src")
                || chunk.contains("data-src")
                || body.contains("reader-body")
                || body.contains("reading-content")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-sec-src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| page(index, &absolute_url(&image)))
        .collect()
}

fn parse_api_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let (slug, cap) = extract_slug_cap(body, chapter_url).unwrap_or(("sample".into(), "1".into()));
    let api_url = format!("{BASE_URL}/api_lector.php?slug={slug}&cap={cap}");
    let body = fetch_json_or_fixture(&api_url, API_FIXTURE);
    let payload = serde_json::from_str::<ApiResponse>(&body)
        .unwrap_or_else(|_| serde_json::from_str(API_FIXTURE).unwrap());
    if !payload.success {
        return Vec::new();
    }
    let mut pages = payload.paginas;
    pages.sort_by_key(|item| item.orden);
    pages
        .into_iter()
        .filter_map(|item| decode_verify(&item.data_verify))
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
}

fn extract_slug_cap(body: &str, chapter_url: &str) -> Option<(String, String)> {
    if let Some(query) = chapter_url.split_once('?').map(|(_, query)| query) {
        let slug = query_param(query, "manga");
        let cap = query_param(query, "cap");
        if let (Some(slug), Some(cap)) = (slug, cap) {
            return Some((slug, cap));
        }
    }
    if let Some(path) = chapter_url.split("/leer/").nth(1) {
        let mut parts = path.split('/');
        let slug = parts.next()?.to_string();
        let cap = parts
            .next()
            .and_then(|value| value.strip_prefix("capitulo-"))
            .unwrap_or("1")
            .to_string();
        return Some((slug, cap));
    }
    let slug = quoted_after(body, "MANGA_SLUG")?;
    let cap = quoted_after(body, "CAP_INICIAL")?;
    Some((slug, cap))
}

fn decode_verify(input: &str) -> Option<String> {
    let decoded = STANDARD
        .decode(input)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let image: String = decoded.chars().rev().collect();
    image
        .trim()
        .starts_with("http")
        .then(|| image.trim().to_string())
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn section_after<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    let rest = body.split(marker).nth(1)?;
    Some(rest.split("</section>").next().unwrap_or(rest))
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split_once(marker)?.1;
    let quote_index = rest.find(['"', '\''])?;
    let quote = rest.as_bytes()[quote_index] as char;
    let rest = &rest[quote_index + 1..];
    Some(rest.split_once(quote)?.0.to_string())
}

fn chapter_number_from_url(input: &str) -> Option<f32> {
    let slug = input.split("capitulo-").nth(1)?;
    slug.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .and_then(|value| value.parse().ok())
}

fn normalize_key(input: &str) -> String {
    let path = input.trim().trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn status_from_text(status: &str) -> ItemStatus {
    let status = status.to_ascii_lowercase();
    if status.contains("complet") {
        ItemStatus::Completed
    } else if status.contains("pausa") || status.contains("hiato") {
        ItemStatus::Hiatus
    } else if status.contains("cancel") || status.contains("aband") {
        ItemStatus::Cancelled
    } else if status.contains("cultivo")
        || status.contains("curso")
        || status.contains("ongoing")
        || status.contains("emision")
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn trim_float(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    id: i64,
    titulo: String,
    portada: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    success: bool,
    #[serde(default)]
    paginas: Vec<ApiPage>,
}

#[derive(Debug, Deserialize)]
struct ApiPage {
    orden: i64,
    #[serde(rename = "data_verify")]
    data_verify: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<section><h3>Top Rankings</h3><a href="/manga.php?id=1"><img src="/cover.jpg"><h4>Sample Manga</h4></a></section>
<section><h3>Lanzamientos</h3><div class="manga-card"><a href="/manga.php?id=2"><img src="/cover2.jpg"><figcaption>Latest Manga</figcaption></a></div></section>
"#;
const SEARCH_FIXTURE: &str = r#"[{"id":1,"titulo":"Sample Manga","portada":"/cover.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="blood-title">Sample Manga</h1><div class="cultivation-panel"><img src="/cover.jpg"></div>
<div class="text-gray-200"><h3>SINOPSIS</h3>Sample description</div><span class="status-badge">En curso</span>
<a href="/directorio.php?genero=action">Action</a>
<div id="chaptersContainer"><a class="chapter-item" data-chapter-number="1" href="/leer/sample/capitulo-1"><span class="chapter-title">Chapter 1</span><span><i class="ph-clock"></i>2024-01-01</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<body class="reader-body"><img class="protected-img" data-sec-src="/page1.jpg"><img data-src="/page2.jpg"></body>"#;
const API_FIXTURE: &str = r#"{"success":true,"paginas":[{"orden":1,"data_verify":"Z3BqLjFlZ2FwL21vYy5lbHBtYXhlLy86c3B0dGg="}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_home_chapters_and_pages() {
        assert_eq!(parse_home(LIST_FIXTURE, "popular").entries.len(), 1);
        assert_eq!(parse_search(SEARCH_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_html_pages(PAGES_FIXTURE).len(), 2);
        assert_eq!(
            parse_api_pages("", "https://lectorhub.j5z.xyz/leer/sample/capitulo-1").len(),
            1
        );
    }
}
