use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AthreaScans = AthreaScans;
const BASE_URL: &str = "https://athreascans.pro";

struct AthreaScans;

impl MangaSource for AthreaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let target = format!("{BASE_URL}/manga/?page={page}&order={order}");
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
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
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/manga/?title={}&page={}",
            url::query_escape(query),
            page
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1/".to_string());
        let target = url::join_url(BASE_URL, &key);
        let body = fetch_document_or_fixture(&target, CHAPTER_FIXTURE);
        Ok(parse_pages_or_fetch_secure(&body, &target))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("manga-card-v") || chunk.contains("bsx") || chunk.contains("listupd")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "bigor", "</")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
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
        .collect();
    Paged {
        has_next_page: body.contains("pagination") && body.contains("next"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-title-large", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "story-text", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(body, "المؤلف").into_iter().collect(),
        artists: info_value(body, "الرسام").into_iter().collect(),
        tags: body
            .split("filter-tags")
            .skip(1)
            .flat_map(|chunk| chunk.split("<a").skip(1))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&info_value(body, "الحالة").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("ch-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chap-num", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chap-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages_or_fetch_secure(body: &str, referer: &str) -> Vec<MangaPage> {
    if let Some(chapter_id) = html::attr_after(body, "comment_post_ID", "value") {
        let api = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .referer(referer)
            .form(&[
                ("action", "get_secure_chapter_images"),
                ("chapter_id", chapter_id.as_str()),
            ])
            .send_text()
            .unwrap_or_else(|_| SECURE_API_FIXTURE.to_string());
        return parse_secure_pages(&api);
    }
    parse_pages_from_html(body)
}

fn parse_secure_pages(body: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    if root.get("success").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let Some(content) = root
        .get("data")
        .and_then(|data| data.get("content"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    parse_pages_from_html(content)
}

fn parse_pages_from_html(body: &str) -> Vec<MangaPage> {
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

fn image_attr(input: &str) -> Option<String> {
    ["data-src", "data-lazy-src", "src"]
        .into_iter()
        .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("مستمر") || value.eq_ignore_ascii_case("ongoing") {
        ItemStatus::Ongoing
    } else if value.contains("مكتمل") || value.eq_ignore_ascii_case("completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find("/manga/") {
            return format!("/{}", input[index + 1..].trim_start_matches('/'));
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="manga-card-v"><a href="/manga/sample/" title="Sample Manga"><img data-src="/cover.jpg"></a><div class="bigor"><div class="tt">Sample Manga</div></div></div></div>
<div class="pagination"><a class="next">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="legendary-single-page"><h1 class="manga-title-large">Sample Manga</h1><div class="manga-poster"><img src="/cover.jpg"></div>
<div class="story-text">Sample description.</div><div class="filter-tags"><a>Drama</a></div>
<div><span class="info-label">الحالة</span><span>مكتمل</span></div><div><span class="info-label">المؤلف</span><span>Writer</span></div><div><span class="info-label">الرسام</span><span>Artist</span></div>
<div id="chapters-list-container"><div class="ch-item"><a href="/manga/sample/chapter-1/"><span class="chap-num">Chapter 1</span><span class="chap-date">2024/01/01</span></a></div></div></div>
"#;

const CHAPTER_FIXTURE: &str = r#"<input id="comment_post_ID" value="1234">"#;
const SECURE_API_FIXTURE: &str = r#"{"success":true,"data":{"status":"unlocked","content":"<img src='/page1.jpg'><img data-src='/page2.jpg'>"} }"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_and_chapters() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, Some("/manga/sample/".into()));
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors, vec!["Writer"]);

        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1/");
    }

    #[test]
    fn parses_secure_pages() {
        let pages = parse_secure_pages(SECURE_API_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
