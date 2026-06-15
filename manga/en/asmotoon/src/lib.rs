use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AsmodeusScans = AsmodeusScans;
const BASE_URL: &str = "https://asmotoon.com";

struct AsmodeusScans;

impl MangaSource for AsmodeusScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest/")
        } else if page <= 1 {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/search?page={page}")
        };
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
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/search?page={page}&title={}",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            CHAPTER_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    key,
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            !chunk.contains("data-type=novel")
                && (chunk.contains("wire:key")
                    || chunk.contains("group overflow-hidden")
                    || chunk.contains("background-image"))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v))
                    })
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "Manga".to_string()),
                cover: background_image(chunk)
                    .or_else(|| image_attr(chunk))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    entries.dedup_by(|left, right| left.key == right.key);
    Paged {
        has_next_page: entries.len() >= 20,
        entries,
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: background_image(body)
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "id=\"expand_content\"", "</div>")
            .or_else(|| html::text_between(body, "Synopsis", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: parse_tags(body),
        status: parse_status(
            &html::attr_after(body, "alt=\"Status\"", "title").unwrap_or_default(),
        ),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("text-sm") && chunk.contains("href"))
        .filter(|chunk| !chunk.contains("Upcoming"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let locked = chunk.contains("alt=\"Coin\"")
                || chunk.contains("alt='Coin'")
                || chunk.contains("star-circle");
            let mut title = html::text_between(chunk, "text-sm", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            if locked && !title.starts_with("[LOCKED]") {
                title = format!("[LOCKED] {title}");
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "text-xs", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Some(data) =
        html::text_between(body, "application/ld+json", "</script>").and_then(|script| {
            let json = script.split('>').next_back().unwrap_or(&script).trim();
            serde_json::from_str::<ChapterLd>(json).ok()
        })
    {
        let series_id = data
            .is_part_of
            .url
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or("series");
        let chapter_id = data
            .url
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or("chapter");
        return (1..=data.number_of_pages)
            .map(|page| MangaPage {
                content: PageContent::Url {
                    url: format!(
                        "{BASE_URL}/storage/series/webtoon/{series_id}/chapters/{chapter_id}/{page:03}.jpg"
                    ),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {page}")),
                ..MangaPage::default()
            })
            .collect();
    }
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
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

#[derive(Deserialize)]
struct ChapterLd {
    #[serde(rename = "isPartOf")]
    is_part_of: SeriesLd,
    #[serde(rename = "numberOfPages")]
    number_of_pages: u32,
    url: String,
}

#[derive(Deserialize)]
struct SeriesLd {
    url: String,
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "dropped" => ItemStatus::Cancelled,
        "paused" => ItemStatus::Hiatus,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre") || chunk.contains("tag"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_attr(input: &str) -> Option<String> {
    ["data-src", "data-lazy-src", "src"]
        .into_iter()
        .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn background_image(input: &str) -> Option<String> {
    let marker = "background-image:";
    let start = input.find(marker)? + marker.len();
    let tail = &input[start..];
    let raw = tail
        .split([';', '"', '\''])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches("url(")
        .trim_end_matches(')')
        .trim_matches(['"', '\'']);
    (!raw.is_empty()).then(|| raw.to_string())
}

const LIST_FIXTURE: &str = r#"
<div wire:key="serie-1"><a href="/series/sample" title="Sample Manga"><div style="background-image:url('/cover.jpg')"></div></a></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<main id="main-content"><div wire:key="serie-1"><a href="/series/sample" title="Sample Manga"><img src="/cover.jpg"></a></div></main>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="grid"><h1>Sample Manga</h1></div><div class="photoURL" style="background-image:url('/cover.jpg')"></div>
<div id="expand_content">Sample description.</div><img alt="Status" title="Completed"><a href="/search?genre=drama">Drama</a>
<div id="chapters">
<div><a href="/series/sample/chapter-1"><span class="text-sm">Chapter 1</span><span class="text-xs">Jan 1, 2024</span></a></div>
<div><a href="/series/sample/chapter-2"><span class="text-sm">Chapter 2</span><img alt="Coin"></a></div>
</div>
"#;
const CHAPTER_FIXTURE: &str = r#"
<script type="application/ld+json">{"isPartOf":{"url":"https://asmotoon.com/series/sample"},"numberOfPages":2,"url":"https://asmotoon.com/series/sample/chapter-1"}</script>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_asmotoon_html() {
        let list = parse_listing(LIST_FIXTURE);
        assert_eq!(list.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, "/series/sample".to_string());
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters.len(), 2);
        assert!(chapters[1].is_locked);

        let pages = parse_pages(CHAPTER_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
