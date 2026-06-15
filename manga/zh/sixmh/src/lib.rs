use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const SOURCE: SixMH = SixMH;
const BASE_URL: &str = "https://www.liumanhua.com";
const AES_KEY: &[u8; 16] = b"9S8$vJnU2ANeSRoF";

struct SixMH;

impl MangaSource for SixMH {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "addtime"
        } else {
            "hits"
        };
        let target = page_path(&format!("/category/order/{order}"), page(&request));
        Ok(parse_listing(&fetch(&target)?))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            let category = filter(filters, "category").unwrap_or("");
            let order = filter(filters, "order").unwrap_or("hits");
            let category = category.trim_matches('/');
            if category.is_empty() {
                page_path(&format!("/category/order/{order}"), page(&request))
            } else {
                page_path(
                    &format!("/category/{category}/order/{order}"),
                    page(&request),
                )
            }
        } else {
            page_path(
                &format!("/search/{}", url::query_escape(&query)),
                page(&request),
            )
        };
        Ok(parse_listing(&fetch(&target)?))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_chapters(&fetch(&absolute(&key))?))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/1.html".to_string());
        parse_pages(&fetch(&absolute(&key))?)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)?),
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

fn fetch(target: &str) -> ExtensionResult<String> {
    client().get(target).browser_document().send_text()
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details(&fetch(&absolute(key))?, key))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<ul")
        .filter(|chunk| chunk.contains("li.title") || chunk.contains("class=\"title\""))
        .filter_map(|chunk| {
            let title_chunk = chunk.split("class=\"title\"").nth(1).unwrap_or(chunk);
            let href = html::attr_after(title_chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(title_chunk, "<a", "</a")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute(&image)),
                url: Some(absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = body.split("class=\"cy_info\"").nth(1).unwrap_or(body);
    let title = html::text_between(info, "cy_title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "六漫画".to_string());
    let status_text =
        html::text_between(info, "cy_xinxi", "</div").map(|value| html::strip_tags(&value));
    CatalogItem {
        key: key.to_string(),
        title,
        cover: html::attr_after(info, "<img", "src").map(|image| absolute(&image)),
        url: Some(absolute(key)),
        authors: anchor_texts(info).into_iter().take(1).collect(),
        tags: tag_texts(info),
        description: html::text_between(info, "comic-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(status_text.as_deref()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter__item")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let name = html::text_between(chunk, "<a", "</a")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(MangaChapter {
                key,
                title: Some(name),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> ExtensionResult<Vec<MangaPage>> {
    let encoded = body
        .split("params = '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .or_else(|| {
            body.split("params='")
                .nth(1)
                .and_then(|rest| rest.split('\'').next())
        })
        .unwrap_or_default();
    let data = decode_data(encoded)?;
    let dto: PageData = serde_json::from_str(&data).map_err(extension_error)?;
    Ok(dto
        .images
        .into_iter()
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: absolute(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            ..MangaPage::default()
        })
        .collect())
}

fn decode_data(encoded: &str) -> ExtensionResult<String> {
    let decoded = STANDARD.decode(encoded).map_err(extension_error)?;
    if decoded.len() < 17 {
        return Err(extension_error(
            "SixMH reader payload is missing IV/ciphertext",
        ));
    }
    let (iv, cipher_text) = decoded.split_at(16);
    let plain = Aes128CbcDec::new(AES_KEY.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
        .map_err(|error| extension_error(format!("{error:?}")))?;
    String::from_utf8(plain).map_err(extension_error)
}

#[derive(Deserialize)]
struct PageData {
    images: Vec<String>,
}

fn extension_error(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError {
        message: error.to_string(),
    }
}

fn tag_texts(input: &str) -> Vec<String> {
    input
        .split("cy_xinxi")
        .nth(2)
        .unwrap_or("")
        .split("</a")
        .filter_map(|part| {
            html::text_between(part, "<a", "<").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn anchor_texts(input: &str) -> Vec<String> {
    input
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            let rest = part.split_once('>')?.1;
            let value = rest.split("</a").next().unwrap_or_default();
            Some(html::strip_tags(value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default() {
        text if text.contains("连载") => ItemStatus::Ongoing,
        text if text.contains("完结") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn has_next_page(body: &str) -> bool {
    let hrefs = body
        .split("Pagination")
        .nth(1)
        .or_else(|| body.split("NewPages").nth(1))
        .unwrap_or(body)
        .split("<a")
        .filter_map(|chunk| html::attr(chunk, "href"))
        .collect::<Vec<_>>();
    hrefs.len() >= 2 && hrefs[hrefs.len() - 1] != hrefs[hrefs.len() - 2]
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn page_path(path: &str, page: u64) -> String {
    let path = path.trim_end_matches('/');
    if page > 1 {
        absolute(&format!("{path}/page/{page}"))
    } else {
        absolute(path)
    }
}

fn absolute(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(input: &str) -> String {
    let value = input.trim();
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    let path = path.strip_prefix("/index.php").unwrap_or(path);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/comic/"))
}

fn push_unique(mut values: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|existing| existing.key == item.key) {
        values.push(item);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_FIXTURE: &str = r#"<div class="cy_list_mh"><ul><li class="title"><a href="/comic/sample">Sample SixMH</a></li><li><img src="/cover.jpg"></li></ul></div><div id="Pagination"><a href="/category/page/1">1</a><a href="/category/page/2">2</a></div>"#;
    const DETAILS_FIXTURE: &str = r#"<div class="cy_info"><div class="cy_title">Sample SixMH</div><div class="cy_info_cover"><a><img class="pic" src="/cover.jpg"></a></div><div class="cy_xinxi"><span><a>Author</a></span><span>完结</span></div><div class="cy_xinxi"><span><a>Action</a></span></div><div class="cy_desc"><p id="comic-description">Summary</p></div></div><ul id="mh-chapter-list-ol-0"><li class="chapter__item"><a href="/comic/sample/1.html">Chapter 1</a></li></ul>"#;
    const PAGES_FIXTURE: &str = "params = 'MDEyMzQ1Njc4OWFiY2RlZv8qdJPLJ9n6ae3FAX1vAQro2LDZl7q95yNF8bWfD00ZnJk1M8BIYX1Rd9/8B3MpnW4Ma4Jv5A7eSbP4P7GpBjg='";

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample SixMH");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = parse_details(DETAILS_FIXTURE, "/comic/sample");
        assert_eq!(item.title, "Sample SixMH");
        assert_eq!(item.authors, vec!["Author"]);
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/comic/sample/1.html");
    }

    #[test]
    fn decrypts_pages() {
        let pages = parse_pages(PAGES_FIXTURE).unwrap();
        assert_eq!(
            pages[0].content,
            PageContent::Url {
                url: "https://www.liumanhua.com/page1.jpg".to_string(),
                context: Some(manga::image_headers(BASE_URL))
            }
        );
    }
}

export_manga_source!(SOURCE);
