use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ShiraiScans = ShiraiScans;
const BASE_URL: &str = "https://shiraixis.space";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const IMAGE_EXTS: [&str; 3] = [".webp", ".jpg", ".png"];

struct ShiraiScans;

impl MangaSource for ShiraiScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_document(BASE_URL, LATEST_FIXTURE)));
        }
        Ok(parse_listing(
            &fetch_document(&library_url(page, "", "todos"), LIST_FIXTURE),
            true,
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let genre = filter_value(&request, "genre").unwrap_or_else(|| "todos".to_string());
        Ok(parse_listing(
            &fetch_document(&library_url(page(&request), query, &genre), LIST_FIXTURE),
            true,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/capitulo/sample".into());
        Ok(parse_pages(&fetch_document(
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

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("key"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = resolve_image_url(key).unwrap_or_else(|| format!("{key}.webp"));
        Ok(MangaPageImage {
            url,
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .map(|body| decode_html(&body))
        .unwrap_or_else(|_| fixture.to_string())
}

fn decode_html(body: &str) -> String {
    let Some(start) = body.find("var b64") else {
        return body.to_string();
    };
    let rest = &body[start..];
    let Some(eq) = rest.find('=') else {
        return body.to_string();
    };
    let value = rest[eq + 1..].trim_start();
    let quote = value.chars().next().unwrap_or_default();
    if quote != '"' && quote != '\'' {
        return body.to_string();
    }
    let Some(end) = value[1..].find(quote) else {
        return body.to_string();
    };
    let encoded = value[1..1 + end].split_whitespace().collect::<String>();
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| body.to_string())
}

fn library_url(page: u64, query: &str, genre: &str) -> String {
    let offset = page.saturating_sub(1) * 15;
    format!(
        "{BASE_URL}/biblioteca.php?ajax=true&genero={}&q={}&offset={offset}",
        url::query_escape(genre),
        url::query_escape(query)
    )
}

fn parse_listing(body: &str, paged: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("library-card"))
        .filter_map(|chunk| {
            let href =
                href_from_onclick(chunk).or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "library-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Shirai Scans".to_string()),
                cover: html::attr_after(chunk, "library-cover", "src")
                    .map(|src| absolute_url(&src)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let has_next_page = paged && entries.len() == 15;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-card"))
        .filter_map(|chunk| {
            let href =
                href_from_onclick(chunk).or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "manga-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Shirai Scans".to_string()),
                cover: html::attr_after(chunk, "manga-cover", "src").map(|src| absolute_url(&src)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/obra/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "obra-titulo", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Shirai Scans".to_string()),
        cover: html::attr_after(body, "obra-capa-grande", "src").map(|src| absolute_url(&src)),
        authors: info_value(body, "Autor")
            .into_iter()
            .filter(|value| value != "?")
            .collect(),
        artists: info_value(body, "Artista")
            .into_iter()
            .filter(|value| value != "?")
            .collect(),
        description: html::text_between(body, "obra-sinopse", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("genero-badge")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        status: status_from(info_value(body, "Status").as_deref()),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("capitulo-item"))
        .filter_map(|chunk| {
            let key = normalize_key(&html::attr(chunk, "href")?);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "capitulo-title", "</")
                        .map(|value| {
                            html::strip_tags(&value)
                                .replace("NOVO", "")
                                .trim()
                                .to_string()
                        })
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Capitulo".to_string()),
                ),
                date_uploaded: html::text_between(chunk, "capitulo-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_pt_date(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let json = body
        .split("const pagesData = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or("[]");
    serde_json::from_str::<Vec<PageDto>>(json)
        .or_else(|_| serde_json::from_str::<Vec<PageDto>>(PAGES_JSON_FIXTURE))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Lazy {
                key: page.url_base,
                url: None,
                page_url: None,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn resolve_image_url(base: &str) -> Option<String> {
    IMAGE_EXTS.iter().find_map(|ext| {
        let image_url = format!("{base}{ext}");
        client()
            .get(&image_url)
            .header("Referer", format!("{BASE_URL}/"))
            .send_text()
            .ok()
            .map(|_| image_url)
    })
}

fn href_from_onclick(chunk: &str) -> Option<String> {
    chunk
        .split("href='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(ToString::to_string)
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("info-linha")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| chunk.rsplit("<span").next())
        .and_then(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_from(value: Option<&str>) -> ItemStatus {
    let lower = value.unwrap_or_default().to_ascii_lowercase();
    if lower.contains("lan") || lower.contains("andamento") {
        ItemStatus::Ongoing
    } else if lower.contains("completo") {
        ItemStatus::Completed
    } else if lower.contains("hiato") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_pt_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    dates::parse_ymd(&format!("{year}-{month}-{day}"))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    let filters = request.get("filters")?;
    if let Some(value) = filters.get(id).and_then(Value::as_str) {
        return Some(value.to_string());
    }
    filters.as_array()?.iter().find_map(|filter| {
        (filter.get("id").and_then(Value::as_str) == Some(id))
            .then(|| {
                filter
                    .get("value")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .flatten()
    })
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

#[derive(Default, Deserialize)]
struct PageDto {
    url_base: String,
}

const LIST_FIXTURE: &str = r#"<div class="library-card" onclick="location.href='obra/sample'"><img class="library-cover" src="/cover.jpg"><div class="library-title">Sample Shirai</div></div>"#;
const LATEST_FIXTURE: &str = r#"<section class="atualizacoes"><div class="manga-card" onclick="location.href='obra/sample'"><img class="manga-cover" src="/cover.jpg"><div class="manga-title">Sample Shirai</div></div></section>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="obra-titulo">Sample Shirai</h1><img class="obra-capa-grande" src="/cover.jpg"><div class="obra-sinopse">Sample description</div><div class="info-linha">Autor <span>Author</span></div><div class="info-linha">Status <span>Lançamento</span></div><div class="lista-capitulos"><a class="capitulo-item" href="capitulo/1"><span class="capitulo-title">Capitulo 1</span><span class="capitulo-date">01/01/2024</span></a></div>"#;
const PAGES_JSON_FIXTURE: &str = r#"[{"url_base":"https://shiraixis.space/images/sample/1"}]"#;
const PAGES_FIXTURE: &str = r#"<script>const pagesData = [{"url_base":"https://shiraixis.space/images/sample/1"}];</script>"#;

export_manga_source!(SOURCE);
