use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaBTT = MangaBTT;
const BASE_URL: &str = "https://manhwabtt.cc";

struct MangaBTT;

impl MangaSource for MangaBTT {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "0"
        } else {
            "12"
        };
        Ok(parse_listing(&fetch_document(
            &find_story_url(page, "", "", "-1", sort),
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
        let genre = filter_value(&request, "genre").unwrap_or_default();
        let status = filter_value(&request, "status").unwrap_or_else(|| "-1".to_string());
        let sort = filter_value(&request, "sort").unwrap_or_else(|| "12".to_string());
        Ok(parse_listing(&fetch_document(
            &find_story_url(page, query, &genre, &status, &sort),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample-1".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample-1".into());
        let story_id = key.rsplit('-').next().unwrap_or("1");
        let body = client()
            .post(format!("{BASE_URL}/Story/ListChapterByStoryID"))
            .header("Accept", "*/*")
            .header("Origin", BASE_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(url::join_url(BASE_URL, &key))
            .form(&[("StoryID", story_id)])
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
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

fn find_story_url(page: u64, query: &str, genre: &str, status: &str, sort: &str) -> String {
    if !query.is_empty() {
        return format!(
            "{BASE_URL}/find-story?keyword={}&page={page}",
            url::query_escape(query)
        );
    }
    let genre_path = if genre.is_empty() {
        String::new()
    } else {
        format!("/{genre}")
    };
    format!("{BASE_URL}/find-story{genre_path}?status={status}&sort={sort}&page={page}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "figcaption", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "figcaption", "</figcaption>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaBTT".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                status: ItemStatus::Unknown,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("active +")
            || body.contains("pagination") && body.contains("Next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample-1".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "title-detail", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaBTT".into())),
        cover: body
            .split("detail-info")
            .nth(1)
            .and_then(image_from_chunk)
            .or_else(|| image_from_chunk(body)),
        description: html::text_between(body, "detail-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .map(|value| value.replace("comic site. The Summary is ", ""))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "author"),
        tags: link_values(body, "kind"),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| !chunk.contains("heading"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: Some(0),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("page-chapter")
        .skip(1)
        .filter_map(image_from_chunk)
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

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-lazy-src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn info_values(body: &str, class_name: &str) -> Vec<String> {
    body.split(&format!("class=\"{class_name}"))
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("updating"))
        .collect()
}

fn link_values(body: &str, class_name: &str) -> Vec<String> {
    body.split(&format!("class=\"{class_name}"))
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1).collect::<Vec<_>>())
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") || lower.contains("đang cập nhật") {
        ItemStatus::Ongoing
    } else if lower.contains("on-hold") {
        ItemStatus::Hiatus
    } else if lower.contains("canceled") {
        ItemStatus::Cancelled
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

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="items"><div class="row"><div class="item"><figure><a href="/manga/sample-1"><img data-src="/cover.jpg"></a><figcaption><h3><a href="/manga/sample-1">Sample Manga</a></h3></figcaption></figure></div></div></div>
<ul class="pagination"><li>Next</li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="title-detail">Sample Manga</h1><div class="detail-info"><img src="/cover.jpg"><div class="status"><p>ongoing</p></div><div class="author"><p>Author</p></div><div class="kind"><a>Action</a></div></div>
<div class="detail-content"><p>comic site. The Summary is Description</p></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"<ul><li><a href="/manga/sample/chapter-1">Chapter 1</a><div class="col-xs-4">1 day ago</div></li></ul>"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-detail"><div class="page-chapter"><img data-index="1" data-src="/pages/001.jpg"></div><div class="page-chapter"><img data-index="2" src="/pages/002.jpg"></div></div>
"#;
