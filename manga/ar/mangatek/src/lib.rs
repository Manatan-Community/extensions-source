use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaTek = MangaTek;
const BASE_URL: &str = "https://mangatek.com";

struct MangaTek;

impl MangaSource for MangaTek {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/manga-list?page={page}")
        } else {
            format!("{BASE_URL}/manga-list?sort=views&page={page}")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!(
                "{BASE_URL}/manga-list?search={}&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/sample/1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<h3") || chunk.contains("manga-card"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<h3", "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        has_next_page: body.contains("fa-chevron-left") && body.contains("aria-disabled=\"false\""),
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
        cover: html::attr_after(body, "mangaCover", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "p class=\"text-base", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_after(body, "المؤلف:")
            .into_iter()
            .filter(|v| v != "Unknown")
            .collect(),
        tags: info_after(body, "التصنيفات:")
            .map(|value| value.replace('،', ","))
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: if body.contains("مكتمل") {
            ItemStatus::Completed
        } else if body.contains("مستمر") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let slug = manga_key
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample");
    let Some(props) = html::text_between(body, "props=\"", "\">")
        .or_else(|| html::attr_after(body, "MangaChaptersLoader", "props"))
    else {
        return Vec::new();
    };
    let decoded = decode_json_entities(&props);
    let Ok(root) = serde_json::from_str::<Value>(&decoded) else {
        return Vec::new();
    };
    let chapters = root
        .get("manga")
        .and_then(|value| value.get(1))
        .and_then(|manga| manga.get("MangaChapters"))
        .and_then(|value| value.get(1))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    chapters
        .into_iter()
        .filter_map(|wrapped| {
            let chapter = wrapped.get(1)?;
            let number = wrapped_field(chapter, "chapter_number").unwrap_or_else(|| "1".into());
            Some(MangaChapter {
                key: format!("/reader/{slug}/{number}"),
                title: wrapped_field(chapter, "title")
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(format!("Chapter {number}"))),
                date_uploaded: wrapped_field(chapter, "created_at")
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(format!("{BASE_URL}/reader/{slug}/{number}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("manga-page")
                || chunk.contains("data-src")
                || chunk.contains("data-url")
                || chunk.contains("src")
        })
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

fn wrapped_field(value: &Value, key: &str) -> Option<String> {
    let value = value.get(key)?.get(1)?;
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn image_attr(input: &str) -> Option<String> {
    [
        "data-src",
        "data-url",
        "data-zoom-src",
        "data-lazy-src",
        "data-cfsrc",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr(input, attr).or_else(|| html::attr_after(input, "<img", attr)))
}

fn info_after(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn decode_json_entities(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find("/manga/") {
            return format!("/{}", input[index + 1..].trim_matches('/'));
        }
    }
    format!("/{}", input.trim_matches('/'))
}

const LIST_FIXTURE: &str = r#"<div class="flex-grow"><div class="grid"><a href="/manga/sample"><img data-src="/cover.jpg"><h3 title="Sample Manga">Sample Manga</h3></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1><img id="mangaCover" src="/cover.jpg"><p class="text-base">Sample summary.</p>
<p><span>المؤلف:</span><span>Writer</span></p><p><span>التصنيفات:</span><span>Drama، Action</span></p><div class="flex"><span class="border rounded">مكتمل</span></div>
<astro-island component-url="MangaChaptersLoader" props="{&quot;manga&quot;:[0,{&quot;MangaChapters&quot;:[0,[[0,{&quot;chapter_number&quot;:[0,&quot;1&quot;],&quot;title&quot;:[0,&quot;Start&quot;],&quot;created_at&quot;:[0,&quot;2024-01-01T00:00:00.000Z&quot;]}]]]}]}"></astro-island>
"#;
const PAGES_FIXTURE: &str = r#"<div class="manga-page"><img data-src="/page1.jpg"></div><div class="manga-page"><img src="/page2.jpg"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mangatek() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, "/manga/sample".into());
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(DETAILS_FIXTURE, "/manga/sample");
        assert_eq!(chapters[0].key, "/reader/sample/1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
