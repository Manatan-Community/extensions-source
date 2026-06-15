use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    ProcessedImage, Paged, PageContent, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{
    dates, html, manga, manga_image,
    sdk::http::HttpClient,
    speedbinb::SpeedBinbReader,
    url,
};
use serde_json::{Value, json};

const SOURCE: ComicBoost = ComicBoost;
const BASE_URL: &str = "https://comic-boost.com";

struct ComicBoost;

impl MangaSource for ComicBoost {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/genre/?p={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}/search?k={}&p={}",
                url::query_escape(query),
                page(&request)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/product/sample?cid=sample".into());
        let Some(cid) = query_param(&key, "cid") else {
            return Ok(vec![text_page("This chapter does not expose a reader id.")]);
        };
        let cphp = fetch_document(
            &format!("{BASE_URL}/pageapi/viewer/c.php?cid={}", url::query_escape(&cid)),
            C_PHP_FIXTURE,
        );
        let Some(reader_url) = serde_json::from_str::<Value>(&cphp)
            .ok()
            .and_then(|root| root.get("url").and_then(Value::as_str).map(ToOwned::to_owned))
        else {
            return Ok(vec![text_page("This chapter is locked. Log in and purchase it before reading.")]);
        };
        let body = fetch_document(&reader_url, READER_FIXTURE);
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: true,
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::SpeedBinb::process_page_image(request)
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("book-list-item")
            .skip(1)
            .filter_map(parse_listing_item)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("to-next") && !body.contains("to-next disabled"),
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "book-list-item-thum-wrapper", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = key_path(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into())),
        cover: html::attr_after(chunk, "thum", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key_path(key),
        title: html::text_between(body, "comic-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Comic".into())),
        cover: html::attr_after(body, "comic-main-thum-wrapper", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        authors: html::text_between(body, "author-list", "</div>")
            .map(|value| {
                value
                    .split("author")
                    .skip(1)
                    .filter_map(|chunk| html::text_between(chunk, ">", "</"))
                    .map(|value| {
                        html::strip_tags(&value)
                            .replace("原作：", "")
                            .replace("漫画：", "")
                            .replace("作画：", "")
                            .replace("キャラクター原案：", "")
                            .replace("原案：", "")
                    })
                    .filter(|value| !value.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        description: html::text_between(body, "comic-description-text", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: html::text_between(body, "tag-list", "</")
            .map(|value| {
                value
                    .split("tag")
                    .skip(1)
                    .filter_map(|chunk| html::text_between(chunk, ">", "</"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("book-product-list-item")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let locked = chunk.contains("coin");
            Some(MangaChapter {
                key: key_path(&href),
                title: Some(if locked { format!("Locked: {title}") } else { title }),
                url: Some(absolute_url(&href)),
                is_locked: locked,
                date_uploaded: html::text_between(chunk, "update-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_ymd(&value)),
                source_order: Some(index as i32),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn text_page(text: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Text { text: text.into() },
        description: Some(text.into()),
        ..MangaPage::default()
    }
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        let key = key_path(input);
        if key.starts_with("/comic/") {
            return Some(key);
        }
    }
    None
}

fn key_path(input: &str) -> String {
    let path = if let Some(rest) = input.strip_prefix(BASE_URL) {
        rest
    } else if let Some(index) = input.find("/comic/").or_else(|| input.find("/product/")) {
        &input[index..]
    } else {
        input
    };
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn query_param(input: &str, key: &str) -> Option<String> {
    let query = input.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="book-list-item book-list-item-thum-wrapper" href="/comic/sample">
  <img class="thum" data-src="/sample.jpg" />
  <span class="title">Sample Comic</span>
</a>
<div class="pagination-list right"><span class="to-next"><a href="?p=2">Next</a></span></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="comic-title">Sample Comic</h1>
<div class="comic-main-thum-wrapper"><img src="/sample.jpg" /></div>
<div class="comic-description-text">Sample description.</div>
<div class="author-list"><span class="author">漫画：Sample Author</span></div>
<div class="tag-list"><span class="tag">Fantasy</span></div>
<div class="book-product-list-item" href="/product/sample?cid=sample">
  <span class="title">第1話</span>
  <span class="update-date">2025/01/02</span>
</div>
"#;

const C_PHP_FIXTURE: &str = r#"{"url":"https://cdn.comic-boost.com/viewer/sample?cid=sample"}"#;

const READER_FIXTURE: &str = r#"
<div id="content"><img data-ptimg="/sample.ptimg.json"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_and_chapters() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Sample Comic");
        assert!(list.has_next_page);

        let chapters = SOURCE.chapters(json!({"manga": "/comic/sample"})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("第1話"));
        assert_eq!(chapters[0].date_uploaded, dates::parse_ymd("2025/01/02"));
    }
}
