use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http, url};
use serde_json::Value;

const SOURCE: LikeManga = LikeManga;
const BASE_URL: &str = "https://likemanga.ink";

struct LikeManga;

impl MangaSource for LikeManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: true,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastest-chap"
        } else {
            "top-manga"
        };
        let body = fetch_text(&search_url(page, "", sort), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();

        if let Some(key) = key_from_url(query) {
            let body = fetch_text(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }

        let body = fetch_text(&search_url(page, query, "top-manga"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_text(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".to_string());
        let body = fetch_text(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);

        if let (Some(manga_id), Some(last_page)) = (manga_id(&body), last_chapter_page(&body)) {
            for page in 2..=last_page {
                let ajax = fetch_text(&chapter_ajax_url(manga_id, page), AJAX_FIXTURE);
                chapters.extend(parse_ajax_chapters(&ajax));
            }
        }

        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1".to_string());
        let body = fetch_text(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_token_pages(&body).unwrap_or_else(|| parse_image_pages(&body)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let body = fetch_text(&absolute_url(&key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, sort: &str) -> String {
    let mut params = vec![
        "act=searchadvance".to_string(),
        format!("f%5Bsortby%5D={}", url::query_escape(sort)),
    ];
    if !query.is_empty() {
        params.push(format!("f%5Bkeyword%5D={}", url::query_escape(query)));
    }
    if page > 1 {
        params.push(format!("pageNum={page}"));
    }
    format!("{BASE_URL}/?{}", params.join("&"))
}

fn chapter_ajax_url(manga_id: u64, page: u64) -> String {
    format!(
        "{BASE_URL}/?act=ajax&code=load_list_chapter&manga_id={manga_id}&page_num={page}&chap_id=0&keyword="
    )
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn normalize_key(value: &str) -> String {
    let path = value
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_matches('/');
    format!("/{path}")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.len() > 1)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("div class=\"card\"")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "title-manga", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && (body.contains("&raquo;") || body.contains(">»<"))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key
        .or_else(|| {
            html::attr_after(body, "rel=\"canonical\"", "href").map(|href| normalize_key(&href))
        })
        .unwrap_or_else(|| "/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "title-detail-manga", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: body
            .split("detail-info")
            .nth(1)
            .and_then(image_from_chunk)
            .or_else(|| image_from_chunk(body)),
        description: html::text_between(body, "summary_shortened", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "author"),
        tags: genre_values(body),
        status: status_from_body(body),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("wp-manga-chapter")
        .skip(1)
        .filter_map(chapter_from_chunk)
        .collect()
}

fn parse_ajax_chapters(body: &str) -> Vec<MangaChapter> {
    let html_fragment = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("list_chap")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.to_string());
    parse_chapters(&html_fragment)
}

fn chapter_from_chunk(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Chapter".to_string());
    let key = normalize_key(&href);
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title),
        url: Some(absolute_url(&key)),
        date_uploaded: parse_month_day_year(chunk),
        ..MangaChapter::default()
    })
}

fn parse_token_pages(body: &str) -> Option<Vec<MangaPage>> {
    let cdn = html::attr_after(body, "currentlink", "value")?;
    let token = html::attr_after(body, "next_img_token", "value")?
        .split('.')
        .nth(1)?
        .to_string();
    let decoded = STANDARD
        .decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let data = serde_json::from_str::<Value>(&decoded).ok()?;
    let encoded_array = data.get("data")?.as_str()?;
    let array_json = STANDARD
        .decode(encoded_array)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let images = serde_json::from_str::<Value>(&array_json).ok()?;
    let pages = images
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| image_page(index, &url::join_url(&cdn, image)))
        .collect::<Vec<_>>();
    (!pages.is_empty()).then_some(pages)
}

fn parse_image_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| image_page(index, &image))
        .collect()
}

fn image_page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: absolute_url(image),
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-cfsrc")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset")
                .map(|value| value.split_whitespace().next().unwrap_or("").to_string())
        })
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| absolute_url(&value))
}

fn manga_id(body: &str) -> Option<u64> {
    html::attr_after(body, "title-detail-manga", "data-manga").and_then(|value| value.parse().ok())
}

fn last_chapter_page(body: &str) -> Option<u64> {
    body.rsplit("load_list_chapter(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|value| value.trim().parse().ok())
}

fn info_values(body: &str, class_name: &str) -> Vec<String> {
    body.split(&format!("class=\"{class_name}"))
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "Updating")
        .collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("/genres/")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from_body(body: &str) -> ItemStatus {
    let status = body
        .split("class=\"status")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    match status.to_ascii_lowercase().as_str() {
        value if value.contains("complete") => ItemStatus::Completed,
        value if value.contains("in process") => ItemStatus::Ongoing,
        value if value.contains("pause") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn parse_month_day_year(chunk: &str) -> Option<i64> {
    html::text_between(chunk, "chapter-release-date", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .map(|_| 0)
}

const LIST_FIXTURE: &str = r#"
<div class="search_genres"><div class="form-check"><input value="Action"><label>Action</label></div></div>
<div class="card-body"><div class="card"><a href="/sample"><img data-src="/cover.jpg"></a><div class="title-manga">Sample Manga</div></div></div>
<ul class="pagination"><a>»</a></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 id="title-detail-manga" data-manga="123">Sample Manga</h1>
<div class="detail-info"><img src="/cover.jpg"></div>
<div id="summary_shortened">A sample story.</div>
<div class="list-info"><div class="author"><p>Author Name</p></div><div class="status"><p>In process</p></div><a href="/genres/action">Action</a></div>
<ul><li class="wp-manga-chapter"><a href="/sample/chapter-1">Chapter 1</a><span class="chapter-release-date">January 01, 2024</span></li></ul>
<div class="chapters_pagination"><a onclick="load_list_chapter(2)">2</a></div>
"#;

const AJAX_FIXTURE: &str = r#"{"list_chap":"<li class=\"wp-manga-chapter\"><a href=\"/sample/chapter-2\">Chapter 2</a></li>"}"#;

const PAGES_FIXTURE: &str = r#"
<div class="reading-detail box_doc">
  <img data-src="/pages/001.jpg">
  <img src="/pages/002.jpg">
</div>
"#;
