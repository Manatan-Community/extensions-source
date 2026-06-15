use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RagnaScans = RagnaScans;
const BASE_URL: &str = "https://lector.ragnascan.xyz";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct RagnaScans;

impl MangaSource for RagnaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "actualizado"
        } else {
            "vistas"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &directory_url(page, "", Some(order), None),
            LIST_FIXTURE,
        )))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &directory_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/obra/sample/capitulo-1".into());
        let page_url = absolute_url(&key);
        Ok(parse_pages(
            &fetch_document_or_fixture(&page_url, PAGES_FIXTURE),
            &page_url,
        ))
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

fn client() -> HttpClient {
    HttpClient::browser()
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

fn directory_url(
    page: u64,
    query: &str,
    forced_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let order = forced_order
        .map(ToString::to_string)
        .or_else(|| filter_value(filters, "orden"))
        .unwrap_or_else(|| "vistas".to_string());
    let mut params = vec![
        ("page".to_string(), page.to_string()),
        ("orden".to_string(), order),
        ("q".to_string(), query.to_string()),
    ];
    for value in filter_values(filters, "generos") {
        params.push(("generos[]".to_string(), value));
    }
    for value in filter_values(filters, "estado") {
        params.push(("estado[]".to_string(), value));
    }
    for value in filter_values(filters, "tipo") {
        params.push(("tipo[]".to_string(), value));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/directorio.php?{query}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("mod-card"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "mod-card-title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Ragna".into()));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("mod-pg-btn") && body.contains("Sig"),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let meta = body.split("meta-table").nth(1).unwrap_or(body);
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Ragna Scans".to_string()),
        cover: html::attr_after(body, "cover-wrapper", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: text_after_labels(body, &["Autor:"]),
        artists: text_after_labels(body, &["Ilustrador:"]),
        tags: meta_values(meta, "genero"),
        description: html::text_between(body, "sinopsisWrapper", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&meta_text(meta, "estado").unwrap_or_default()),
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
        .filter(|chunk| !chunk.contains("locked-neon") && !chunk.contains("ph-lock-key"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "chapter-item-title", "</")
                .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value).trim_end_matches(".00").to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::text_between(chunk, "chapter-item-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_spanish_date(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("page-container")
                || chunk.contains("data-verify")
                || chunk.contains("src")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-verify")
                .and_then(|value| decode_verify(&value))
                .or_else(|| image_attr(chunk))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(page_url)),
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_verify(input: &str) -> Option<String> {
    let bytes = decode_base64(input)?;
    let decoded = String::from_utf8(bytes).ok()?;
    let value = decoded.chars().rev().collect::<String>();
    Some(if value.starts_with("http") || value.starts_with("//") {
        value
    } else {
        format!("{BASE_URL}{value}")
    })
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut out = Vec::new();
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        bit_count += 6;
        while bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    Some(out)
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset").map(|value| {
                value
                    .split_whitespace()
                    .next()
                    .unwrap_or(&value)
                    .to_string()
            })
        })
        .or_else(|| html::attr(chunk, "src"))
}

fn filter_value(filters: Option<&Value>, id: &str) -> Option<String> {
    let value = filters?.get(id)?;
    value.as_str().map(ToString::to_string)
}

fn filter_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(id)) else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(|item| item.as_str().map(ToString::to_string))
            .collect();
    }
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn meta_values(body: &str, label: &str) -> Vec<String> {
    body.split("meta-row")
        .filter(|chunk| chunk.to_ascii_lowercase().contains(label))
        .flat_map(|chunk| link_values(chunk))
        .collect()
}

fn meta_text(body: &str, label: &str) -> Option<String> {
    body.split("meta-row")
        .find(|chunk| chunk.to_ascii_lowercase().contains(label))
        .and_then(|chunk| html::text_between(chunk, "meta-value", "</"))
        .map(|value| html::strip_tags(&value))
}

fn link_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn text_after_labels(body: &str, labels: &[&str]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| {
            let rest = body.split(label).nth(1)?;
            Some(html::strip_tags(rest.split('<').next().unwrap_or_default()))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("finalizado") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("pausado") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelado") {
        ItemStatus::Cancelled
    } else if lower.contains("emis") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_spanish_date(value: &str) -> Option<i64> {
    let clean = value.replace(',', "");
    let mut parts = clean.split_whitespace();
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = spanish_month(parts.next()?)?;
    let year = parts.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn spanish_month(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "enero" => Some(1),
        "febrero" => Some(2),
        "marzo" => Some(3),
        "abril" => Some(4),
        "mayo" => Some(5),
        "junio" => Some(6),
        "julio" => Some(7),
        "agosto" => Some(8),
        "septiembre" => Some(9),
        "octubre" => Some(10),
        "noviembre" => Some(11),
        "diciembre" => Some(12),
        _ => None,
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="mod-grid"><a class="mod-card" href="/obra/sample"><img class="mod-card-cover" src="/cover.jpg"><span class="mod-card-title">Sample Ragna</span></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Ragna</h1><div class="cover-wrapper"><img src="/cover.jpg"></div><div id="sinopsisWrapper"><p>Summary.</p></div><div class="meta-table"><div class="meta-row"><span class="meta-label">Genero</span><span class="meta-value"><a>Drama</a></span></div><div class="meta-row"><span class="meta-label">Estado</span><span class="meta-value">En emision</span></div></div><div id="chaptersContainer"><a class="chapter-item" href="/obra/sample/capitulo-1"><div class="chapter-item-title"><h4>Capitulo 1.00</h4></div><span class="chapter-item-date">01 enero, 2024</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="pagesContainer"><div class="page-container"><img src="/page1.jpg"></div><div class="page-container"><img data-verify="Z3BqLjIv"></div></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ragna_fixtures() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
    }
}
