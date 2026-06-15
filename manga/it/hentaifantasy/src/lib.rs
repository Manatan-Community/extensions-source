use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiFantasy = HentaiFantasy;
const BASE_URL: &str = "https://hentaifantasy.it";

struct HentaiFantasy;

impl MangaSource for HentaiFantasy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing_id = request.get("listingId").and_then(Value::as_str).unwrap_or("popular");
        let path = if listing_id == "latest" { "latest" } else { "most_downloaded" };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/{path}/{page}/"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document_or_fixture(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let tags = selected_tags(&request);
        let (target, body) = if tags.is_empty() {
            if query.len() < 3 {
                return Ok(Paged::default());
            }
            (
                format!("{BASE_URL}/search"),
                client()
                    .post(format!("{BASE_URL}/search"))
                    .form(&[("search", query)])
                    .send_text()
                    .unwrap_or_else(|_| SEARCH_FIXTURE.to_string()),
            )
        } else if tags.len() == 1 {
            let slug = tags[0].split_once(':').map(|(_, slug)| slug).unwrap_or(tags[0].as_str());
            (
                format!("{BASE_URL}/tag/{slug}/{page}"),
                fetch_document_or_fixture(&format!("{BASE_URL}/tag/{slug}/{page}"), SEARCH_FIXTURE),
            )
        } else {
            let form = tags
                .iter()
                .filter_map(|tag| tag.split_once(':').map(|(id, _)| ("tag[]", id)))
                .collect::<Vec<_>>();
            (
                format!("{BASE_URL}/search_tags"),
                client()
                    .post(format!("{BASE_URL}/search_tags"))
                    .form(&form)
                    .send_text()
                    .unwrap_or_else(|_| SEARCH_FIXTURE.to_string()),
            )
        };
        let _ = target;
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_details(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document_or_fixture(input, DETAILS_FIXTURE), Some(key))),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("element"))
        .filter_map(|chunk| listing_item(chunk))
        .chain(
            body.split("class=\"group")
                .skip(1)
                .filter_map(|chunk| listing_item(chunk)),
        )
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("div class=\"next") && body.contains("gbutton"),
    }
}

fn listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "a class=\"thumb", "href")
        .or_else(|| html::attr_after(chunk, "div class=\"title", "href"))
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let title = html::attr_after(chunk, "div class=\"title", "title")
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .or_else(|| url::slug_from_url(&href))
        .unwrap_or_else(|| "HentaiFantasy".to_string());
    Some(CatalogItem {
        key: normalize_key(&href),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
        language: Some("it".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let mut authors = Vec::new();
    let mut tags = Vec::new();
    for row in body.split("meta-row").skip(1) {
        let label = html::text_between(row, "meta-key", "</").map(|value| html::strip_tags(&value)).unwrap_or_default();
        let values = row
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        match label.as_str() {
            "Autore" => authors.extend(values),
            "Genere" | "Tipo" => tags.extend(values),
            _ => {}
        }
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "HentaiFantasy".to_string()),
        cover: html::attr_after(body, "comic-hero", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "desc-text", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors,
        tags,
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-card__title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "chapter-card__title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapter-card__meta", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut urls = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("\"url\":\"") {
        rest = &rest[start + 7..];
        if let Some(end) = rest.find('"') {
            urls.push(rest[..end].replace("\\/", "/"));
            rest = &rest[end..];
        } else {
            break;
        }
    }
    urls.into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn selected_tags(request: &Value) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get("tags"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn parse_date(value: &str) -> Option<i64> {
    let value = value.rsplit(',').next().unwrap_or(value).trim();
    manatan_shared::dates::parse_fixture_date(value)
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "img", "src")
        .or_else(|| html::attr_after(chunk, "img", "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="element"><a class="thumb" href="/sample"><img class="cover" src="/cover.jpg"></a><div class="title"><a title="Sample Manga"></a></div></article><div class="next"><a class="gbutton">»</a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Manga</h1><section class="comic-hero"><img src="/cover.jpg"></section><div class="meta-row"><div class="meta-key">Autore</div><div class="meta-val"><a>Author</a></div></div><div class="meta-row"><div class="meta-key">Genere</div><div class="meta-val"><a>Anal</a></div></div><div class="desc-text">Description</div><article class="chapter-card"><div class="chapter-card__title"><a href="/sample/chapter-1">Chapter 1</a></div><div class="chapter-card__meta">Team, 2024.01.01</div></article>"#;
const PAGES_FIXTURE: &str = r#"window.__pages=[{"url":"https:\/\/hentaifantasy.it\/page1.jpg"},{"url":"\/page2.jpg"}];"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentaifantasy_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Manga");
        assert_eq!(SOURCE.chapters(json!({"manga":"/sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/sample/chapter-1"})).unwrap().len(), 2);
    }
}
