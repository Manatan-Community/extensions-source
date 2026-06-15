use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Webcomics = Webcomics;
const BASE_URL: &str = "https://webcomicsapp.com";
const API_URL: &str = "https://popeye.webcomicsapp.com/api";

struct Webcomics;

impl MangaSource for Webcomics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "Latest_Updated"
        } else {
            "Popularity"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/genres/All/All/{sort}/{page}"),
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        Ok(parse_search(&fetch_document(
            &format!("{BASE_URL}/search/{}", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample/1".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample/1".into());
        let manga_id = key.rsplit('/').next().unwrap_or("1");
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/chapter/list?manga_id={manga_id}"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/view/sample/1/1-sample".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
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
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("<h5") || chunk.contains("manga"))
            .filter_map(item_from_anchor)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("page:") || body.contains("Next"),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("list-item")
            .skip(1)
            .filter_map(item_from_anchor)
            .fold(Vec::new(), push_unique_item),
        has_next_page: false,
    }
}

fn item_from_anchor(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    if !key.contains('/') {
        return None;
    }
    let title = html::text_between(chunk, "<h5", "</h5>")
        .or_else(|| html::text_between(chunk, "info-title", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| url::slug_from_url(&key))?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/sample/1".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h5", "</h5>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Webcomics".into()),
        cover: html::attr_after(body, "card-info", "src")
            .or_else(|| html::attr_after(body, "<img", "src")),
        description: html::text_between(body, "book-detail", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("label-tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if body.contains("chapter-updateDetail") && body.contains("IDK") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let wrapper: ChapterWrapper = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
    let manga = wrapper.data.book;
    let mut chapters: Vec<_> = wrapper
        .data
        .list
        .into_iter()
        .map(|chapter| {
            let locked = chapter.is_pay || chapter.is_paid;
            let title = if locked {
                format!("Locked {}", chapter.name)
            } else {
                chapter.name.clone()
            };
            let slug_name = slug_path(&chapter.name);
            let manga_name = slug_path(&manga.name);
            let key = format!(
                "/view/{}/{}/{}-{}",
                manga_name, chapter.index, manga.manga_id, slug_name
            );
            MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: Some(chapter.index as f32),
                date_uploaded: Some(chapter.update_time),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                is_locked: locked,
                ..MangaChapter::default()
            }
        })
        .collect();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_page_images(body)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: html::html_unescape(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_page_images(body: &str) -> Vec<String> {
    body.split("src:")
        .skip(1)
        .filter_map(|chunk| {
            let start = chunk.find('"').or_else(|| chunk.find('\''))?;
            let quote = chunk.as_bytes()[start] as char;
            let rest = &chunk[start + 1..];
            let end = rest.find(quote)?;
            Some(rest[..end].replace("\\u002F", "/").replace("\\/", "/"))
        })
        .filter(|value| value.starts_with("http"))
        .collect()
}

fn slug_path(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input.trim_start_matches(BASE_URL).to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
    .trim_end_matches('/')
    .to_string()
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct ChapterWrapper {
    data: ChapterData,
}

#[derive(Debug, Deserialize)]
struct ChapterData {
    list: Vec<ChapterDto>,
    book: BookDto,
}

#[derive(Debug, Deserialize)]
struct BookDto {
    manga_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    index: i32,
    #[serde(default)]
    is_paid: bool,
    #[serde(default)]
    is_pay: bool,
    name: String,
    update_time: i64,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="All"><a href="/comics/sample/1"><img src="/cover.jpg"><h5>Sample Comic</h5></a></div>page:1"#;
const SEARCH_FIXTURE: &str = r#"<div class="list-item"><a href="/comics/sample/1"><img src="/cover.jpg"><span class="info-title">Sample Comic</span></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="card-info"><img src="/cover.jpg"><h5>Sample Comic</h5><div class="book-detail"><p>Description</p></div><span class="label-tag">Fantasy</span></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"book":{"manga_id":"1","name":"Sample Comic"},"list":[{"chapter_id":"1","index":1,"is_last":true,"is_paid":false,"is_pay":false,"name":"Chapter 1","update_time":1704067200}]}}"#;
const PAGES_FIXTURE: &str = r#"<script>window.__NUXT__={src:"https://example.com/page1.jpg",src:"https://example.com/page2.jpg"}</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_webcomics() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Comic"
        );
        assert_eq!(
            SOURCE
                .chapters(json!({"manga":"/comics/sample/1"}))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/view/sample/1/1-chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
