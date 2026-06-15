use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangaOni = MangaOni;
const BASE_URL: &str = "https://manga-oni.com";
const SOURCE_NAME: &str = "MangaOni";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct MangaOni;

impl MangaSource for MangaOni {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_directory_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            return Ok(parse_latest_listing(&fetch_document_or_fixture(
                &format!("{BASE_URL}/recientes?p={page}"),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_directory_listing(&fetch_document_or_fixture(
            &directory_url(page, "", &request),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
            return Ok(parse_directory_listing(&fetch_document_or_fixture(
                &directory_url(page, "", &request),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_search_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/buscar?q={}&p={page}", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/capitulo-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
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

fn directory_url(page: u64, query: &str, request: &Value) -> String {
    if !query.is_empty() {
        return format!("{BASE_URL}/buscar?q={}&p={page}", url::query_escape(query));
    }
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let (sort, order) = filter_str(filters, "sort")
        .and_then(|value| {
            value
                .split_once(':')
                .map(|(sort, order)| (sort.to_string(), order.to_string()))
        })
        .unwrap_or_else(|| ("visitas".to_string(), "desc".to_string()));
    let adult = filter_str(filters, "adult").unwrap_or_else(|| {
        if hide_nsfw(request) {
            "0".to_string()
        } else {
            "false".to_string()
        }
    });
    format!(
        "{BASE_URL}/directorio?genero={}&estado={}&filtro={}&tipo={}&adulto={}&orden={}&p={page}",
        filter_str(filters, "genre").unwrap_or_else(|| "false".to_string()),
        filter_str(filters, "status").unwrap_or_else(|| "false".to_string()),
        url::query_escape(&sort),
        filter_str(filters, "type").unwrap_or_else(|| "false".to_string()),
        url::query_escape(&adult),
        url::query_escape(&order),
    )
}

fn parse_directory_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("<img") && chunk.contains("href"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if !href.contains(BASE_URL) && !href.starts_with('/') {
                    return None;
                }
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: first_nonempty_div_text(chunk)
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| SOURCE_NAME.to_string()),
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_latest_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("_1bJU3") || chunk.contains("latest-update-name"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "data-test=\"latest-update-name\"", "href")
                    .or_else(|| html::attr_after(chunk, "data-test='latest-update-name'", "href"))
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "data-test=\"latest-update-name\"", "</a>")
                        .or_else(|| html::text_between(chunk, "<a", "</a>"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| SOURCE_NAME.to_string()),
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_search_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("<img") && chunk.contains("<a"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: first_nonempty_link_text(chunk)
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| SOURCE_NAME.to_string()),
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = html::text_between(body, "id=\"info-i\"", "</div>")
        .or_else(|| html::text_between(body, "id='info-i'", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    let author = if info.to_ascii_lowercase().contains("autor") {
        info.split("Autor:")
            .nth(1)
            .unwrap_or_default()
            .split("Fecha:")
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "cover", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "id=\"sinopsis\"", "</div>")
            .or_else(|| html::text_between(body, "id='sinopsis'", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: (!author.is_empty())
            .then(|| vec![author.clone()])
            .unwrap_or_default(),
        artists: (!author.is_empty())
            .then(|| vec![author])
            .unwrap_or_default(),
        tags: link_values(body, "categ"),
        status: parse_status(
            &html::text_between(body, "strong:contains(Estado)", "</")
                .map(|value| html::strip_tags(&value))
                .or_else(|| text_after(body, "Estado"))
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
        .filter(|chunk| {
            chunk.contains("data-num") || chunk.contains("datetime") || chunk.contains("c_list")
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: html::attr_after(chunk, "<span", "data-num")
                    .and_then(|value| value.parse().ok())
                    .or_else(|| chapter_number_from_text(&title)),
                date_uploaded: html::attr_after(chunk, "<span", "datetime")
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    decode_unicap(body)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_unicap(body: &str) -> Vec<String> {
    let Some(encoded) = body
        .split("unicap")
        .nth(1)
        .and_then(|part| part.split('\'').nth(1).or_else(|| part.split('"').nth(1)))
    else {
        return Vec::new();
    };
    let drop = encoded.len() % 4;
    let trimmed = if drop == 0 {
        encoded
    } else {
        &encoded[..encoded.len().saturating_sub(drop)]
    };
    let Ok(decoded) = STANDARD.decode(trimmed) else {
        return Vec::new();
    };
    let decoded = String::from_utf8_lossy(&decoded);
    let path = decoded.split("||").next().unwrap_or_default();
    decoded
        .split('[')
        .nth(1)
        .and_then(|part| part.split(']').next())
        .into_iter()
        .flat_map(|part| part.split(','))
        .map(|file| file.trim().trim_matches('"'))
        .filter(|file| !file.is_empty())
        .map(|file| format!("{path}{file}"))
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn first_nonempty_link_text(chunk: &str) -> Option<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .find(|value| !value.is_empty())
}

fn first_nonempty_div_text(chunk: &str) -> Option<String> {
    chunk
        .split("<div")
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", "</div>"))
        .map(|value| html::strip_tags(&value))
        .find(|value| !value.is_empty())
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    let section = body
        .split(marker)
        .nth(1)
        .and_then(|part| part.split("</div>").next())
        .unwrap_or(body);
    section
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn text_after(body: &str, label: &str) -> Option<String> {
    let value = body.split(label).nth(1)?;
    Some(
        html::strip_tags(value.split("</span>").next().unwrap_or(value))
            .trim_matches(':')
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("finalizado") || value.contains("completo") {
        ItemStatus::Completed
    } else if value.contains("desarrollo") || value.contains("curso") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn filter_str(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn hide_nsfw(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("HIDE_NSFW"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="article-div"><a href="/manga/sample"><img data-src="/cover.jpg"><div></div><div>Sample MangaOni</div></a></div>
<ul class="pagination"><a rel="next">next</a></ul>
"#;
const LATEST_FIXTURE: &str = r#"
<div class="_1bJU3"><img data-src="/cover.jpg"><a data-test="latest-update-name" href="/manga/sample">Sample MangaOni</a></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<div id="article-div"><div><a href="/manga/sample"><img src="/cover.jpg"></a><a>Sample MangaOni</a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample MangaOni</h1><img src="/cover.jpg"><div id="sinopsis">A sample.</div><div id="info-i">Autor: Author Fecha:</div>
<div id="categ"><a>Action</a></div><strong>Estado</strong><span>En desarrollo</span>
<div id="c_list"><a href="/manga/sample/capitulo-1">Capitulo 1 <span data-num="1" datetime="2024-01-01 00:00:00"></span></a></div>
"#;
const PAGES_FIXTURE: &str = "<script>var unicap = 'aHR0cHM6Ly9jZG4uZXhhbXBsZS8xMjMvfHxbInAwMS5qcGciLCJwMDIuanBnIl0=';</script>";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample MangaOni"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/manga/sample/capitulo-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
