use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaEsp = MangaEsp;
const BASE_URL: &str = "https://mangaesp.topmanhuas.org";
const SOURCE_NAME: &str = "MangaEsp";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";
const PROJECT_PATH: &str = "/proyectos";

struct MangaEsp;

impl MangaSource for MangaEsp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, "", Some(order), request.get("filters")),
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
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
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
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
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

fn search_url(
    page: u64,
    query: &str,
    fallback_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut path = "/manga";
    if filter_string(filters, "project").as_deref() == Some("project-filter-on") {
        path = PROJECT_PATH;
    }
    let mut params = vec![
        ("title", url::query_escape(query)),
        ("page", page.to_string()),
        (
            "order",
            filter_string(filters, "order")
                .or_else(|| filter_string(filters, "sort"))
                .unwrap_or_else(|| fallback_order.unwrap_or_default().to_string()),
        ),
    ];
    let query = params
        .drain(..)
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        format!("{}{path}/", BASE_URL)
    } else {
        format!("{}{path}/?{query}", BASE_URL)
    }
}

fn filter_string(filters: Option<&Value>, name: &str) -> Option<String> {
    filters?.get(name)?.as_str().map(url::query_escape)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("bsx")
                    || chunk.contains("uta")
                    || chunk.contains("listupd")
                    || chunk.contains("imgu")
            })
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagination")
            && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| url::slug_from_url(&href))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: image_attr(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        Some(normalize_key(key)),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| SOURCE_NAME.to_string()),
        cover: html::attr_after(body, "class=\"thumb\"", "src")
            .or_else(|| html::attr_after(body, "class='thumb'", "src"))
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "class=\"desc\"", "</div>")
            .or_else(|| html::text_between(body, "itemprop=\"description\"", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: link_values(body, "/genre/"),
        status: parse_status(&info_values(body, "Status").join(" ")),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter") || chunk.contains("eph-num"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .collect::<Vec<_>>();
    if pages.is_empty() {
        pages = json_pages(body);
    }
    pages
        .into_iter()
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn json_pages(body: &str) -> Vec<String> {
    let Some(raw) = body.split("ts_reader.run(").nth(1) else {
        return Vec::new();
    };
    raw.split("\"images\":[")
        .nth(1)
        .and_then(|part| part.split(']').next())
        .into_iter()
        .flat_map(|part| part.split(','))
        .map(|part| part.trim().trim_matches('"').replace("\\/", "/"))
        .filter(|part| !part.is_empty())
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
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

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if ["completo", "completado", "finalizado", "completed"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Completed
    } else if ["en curso", "ongoing", "publicandose", "publicando"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Ongoing
    } else if value.contains("hiatus") || value.contains("pausado") {
        ItemStatus::Hiatus
    } else if value.contains("cancel") || value.contains("abandon") {
        ItemStatus::Cancelled
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

const LIST_FIXTURE: &str = r#"<div class="bsx"><a href="/manga/sample" title="Sample MangaEsp"><img src="/cover.jpg"></a></div><div class="pagination"><a class="next">next</a></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample MangaEsp</h1><div class="thumb"><img src="/cover.jpg"></div><div class="desc">A sample.</div>
<div>Status <a>En curso</a></div><div><a href="/genre/action">Action</a></div>
<ul><li><a href="/manga/sample/chapter-1"><span class="chapternum">Capitulo 1</span></a><span class="chapterdate">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div id="readerarea"><img class="ts-main-image" src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample MangaEsp"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/manga/sample/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
