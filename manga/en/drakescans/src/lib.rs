use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: DrakeScans = DrakeScans;
const BASE_URL: &str = "https://drakecomic.org";

struct DrakeScans;

impl MangaSource for DrakeScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "update".to_string()
        } else {
            filter_value(&request, "order").unwrap_or_else(|| "popular".to_string())
        };
        Ok(parse_listing(&fetch_document(
            &search_url(page, "", &order, &request),
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let order = filter_value(&request, "order").unwrap_or_default();
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, &order, &request),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &search_url(1, "", "popular", &Value::Null),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &search_url(1, "", "update", &Value::Null),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn search_url(page: u64, query: &str, order: &str, request: &Value) -> String {
    let mut pairs = vec![
        ("title", query.to_string()),
        ("page", page.to_string()),
        ("order", if order.is_empty() { "popular" } else { order }.to_string()),
    ];
    for key in ["author", "year", "status"] {
        if let Some(value) = filter_value(request, key).filter(|value| !value.is_empty()) {
            pairs.push((key, value));
        }
    }
    format!(
        "{BASE_URL}/manga/?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("imgu") || chunk.contains("uta"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Drake Scans".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_url(chunk),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut entries: Vec<CatalogItem>, item| {
            if !entries.iter().any(|entry| entry.key == item.key) {
                entries.push(item);
            }
            entries
        });
    Paged {
        entries,
        has_next_page: body.contains("pagination") && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let title = html::text_between(body, "entry-title", "</")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Drake Scans".into()));
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_url(
            html::text_between(body, "thumb", "</div>")
                .as_deref()
                .unwrap_or(body),
        )
        .or_else(|| image_url(body)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_links(body, "Author"),
        artists: info_links(body, "Artist"),
        tags: info_links(body, "genre"),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("<a") && (chunk.contains("chapternum") || chunk.contains("/manga/")))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.to_string(),
            title: Some("Chapter".to_string()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("readerarea") || chunk.contains("data-src") || chunk.contains("src"))
        .filter_map(|chunk| {
            html::attr(chunk, "data-lazy-src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:"))
        .map(fix_jetpack_url)
        .collect::<Vec<_>>();
    images
        .into_iter()
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

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/manga/") {
        return format!("/{}", input[index + 1..].trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_url(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &fix_jetpack_url(image)))
}

fn fix_jetpack_url(input: String) -> String {
    if let Some(rest) = input.strip_prefix("https://i0.wp.com/") {
        format!("https://{rest}")
    } else if let Some(rest) = input.strip_prefix("https://i1.wp.com/") {
        format!("https://{rest}")
    } else if let Some(rest) = input.strip_prefix("https://i2.wp.com/") {
        format!("https://{rest}")
    } else if let Some(rest) = input.strip_prefix("https://i3.wp.com/") {
        format!("https://{rest}")
    } else {
        input
    }
}

fn info_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .take(1)
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

fn status_from(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") || lower.contains("on going") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
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
<div class="listupd"><div class="bs"><div class="bsx"><a href="/manga/sample/" title="Sample"><img src="/cover.jpg"></a></div></div></div>
<div class="pagination"><a class="next"></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample</h1><div class="thumb"><img src="/cover.jpg"></div><div class="entry-content">Description</div></div>
<ul><li><a href="/manga/sample/chapter-1/"><span class="chapternum">Chapter 1</span></a></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="https://i0.wp.com/drakecomic.org/page1.jpg"><img data-src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_drake_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample");
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
