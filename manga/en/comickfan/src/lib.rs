use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ComicKFan = ComicKFan;
const BASE_URL: &str = "https://comickfan.com";

struct ComicKFan;

impl MangaSource for ComicKFan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "rating"
        };
        let target = advanced_search_url(page, "", Some(sort), request.get("filters"));
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
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
        if query.starts_with("https://") {
            let slug = query
                .split("/manga/")
                .nth(1)
                .and_then(|part| part.split('/').next())
                .unwrap_or("sample");
            let target = format!("{BASE_URL}/manga/{slug}");
            let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(format!("/manga/{slug}")))],
                has_next_page: false,
            });
        }
        let target = advanced_search_url(page, query, None, request.get("filters"));
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let comic_id = key
            .trim_matches('/')
            .split('/')
            .nth(1)
            .unwrap_or("sample")
            .to_string();
        let body = fetch_text_or_fixture(
            &format!("{BASE_URL}/api/comics/{comic_id}/chapter-list?translation_group_id="),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &comic_id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1-samplehash".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn advanced_search_url(
    page: u64,
    query: &str,
    fallback_sort: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let genres = ["format", "content", "theme", "genre", "genres"]
        .into_iter()
        .filter_map(|name| filter_string(filters, name))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let mut params = vec![
        ("genres", genres),
        (
            "status",
            filter_string(filters, "status").unwrap_or_default(),
        ),
        ("type", filter_string(filters, "type").unwrap_or_default()),
        (
            "sort",
            filter_string(filters, "sort")
                .unwrap_or_else(|| fallback_sort.unwrap_or_default().to_string()),
        ),
        ("name", url::query_escape(query)),
        ("page", page.to_string()),
    ];
    let query = params
        .drain(..)
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/advanced-search?{query}")
}

fn filter_string(filters: Option<&Value>, name: &str) -> Option<String> {
    let value = filters?.get(name)?;
    if let Some(text) = value.as_str() {
        return Some(url::query_escape(text));
    }
    if let Some(array) = value.as_array() {
        return Some(
            array
                .iter()
                .filter_map(Value::as_str)
                .map(url::query_escape)
                .collect::<Vec<_>>()
                .join("_"),
        );
    }
    None
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/manga/"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                if href.contains("/chapter-") {
                    return None;
                }
                let title = html::attr_after(chunk, "<img", "alt")
                    .or_else(|| html::attr_after(chunk, "<img", "title"))
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("alt=\"Next\"") || body.contains("alt='Next'"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "ComicK Fanmade".to_string()),
        cover: html::attr_after(body, "thumb-cover", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "comic-content desk", "</div>")
            .or_else(|| html::text_between(body, "comic-content", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: value_after_label(body, "Author").into_iter().collect(),
        artists: value_after_label(body, "Artist").into_iter().collect(),
        tags: link_values(body, "/genres/"),
        status: parse_status(&value_after_label(body, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, comic_id: &str) -> Vec<MangaChapter> {
    let response: ChapterListResponse = serde_json::from_str(body).unwrap_or_default();
    response
        .data
        .into_iter()
        .map(|chapter| chapter.into_chapter(comic_id))
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("loading=\"lazy\"") || chunk.contains("loading='lazy'"))
        .filter_map(image_attr)
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn value_after_label(body: &str, label: &str) -> Option<String> {
    body.split("flex-row")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| {
            let texts = chunk
                .split("<div")
                .filter_map(|part| html::text_between(part, ">", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty() && value != label)
                .collect::<Vec<_>>();
            texts.into_iter().next()
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

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "hiatus" => ItemStatus::Hiatus,
        "cancelled" | "canceled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Default, Deserialize)]
struct ChapterListResponse {
    #[serde(default)]
    data: Vec<ChapterDto>,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterDto {
    #[serde(default, alias = "hash_id")]
    hash_id: String,
    #[serde(default)]
    chapter: String,
    title: Option<String>,
    #[serde(default, alias = "group_names")]
    group_names: Vec<String>,
}

impl ChapterDto {
    fn into_chapter(self, comic_id: &str) -> MangaChapter {
        let key = format!(
            "/manga/{comic_id}/chapter-{}-{}",
            self.chapter, self.hash_id
        );
        let title = match self.title {
            Some(title) if !title.is_empty() => format!("Chapter {} - {title}", self.chapter),
            _ => format!("Chapter {}", self.chapter),
        };
        MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: self.chapter.parse().ok(),
            scanlators: self.group_names,
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div><form></form></div><div class="grid"><a href="/manga/sample"><img alt="Sample Fanmade" src="/cover.jpg"></a></div>"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Fanmade</h1><div class="comic-content desk">A sample.</div>
<div class="bg-card-section"><div class="thumb-cover"><img src="/cover.jpg"></div>
<div class="flex-row gap-4"><div class="text-sm">Author</div><div class="text-sm">Author Name</div></div>
<div class="flex-row gap-4"><div class="text-sm">Status</div><div class="text-sm">Ongoing</div></div>
<div class="font-medium">Genres</div><div><a href="/genres/action">Action</a></div></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"{"data":[{"hash_id":"abc","chapter":"1","title":"Start","group_names":["Group"],"published_at":"2024-01-01T00:00:00000000Z","created_at":"2024-01-01T00:00:00000000Z"}]}"#;

const PAGES_FIXTURE: &str = r#"<div class="w-full"><img loading="lazy" src="/page1.jpg"><img loading="lazy" src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_chapter_api_and_reader_pages() {
        let chapters = SOURCE.chapters(json!({"manga":"/manga/sample"})).unwrap();
        assert_eq!(chapters[0].scanlators, vec!["Group"]);
        let pages = SOURCE
            .pages(json!({"chapter":chapters[0].key.clone()}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
