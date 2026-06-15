use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: TruyenTVN = TruyenTVN;
const BASE_URL: &str = "https://truyentvn.net";
const AJAX_PATH: &str = "/wp-admin/admin-ajax.php";

struct TruyenTVN;

impl MangaSource for TruyenTVN {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "/moi-cap-nhat"
        } else {
            "/xem-nhieu-nhat"
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &paged_url(path, page),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = ajax_client()
                .post(format!("{BASE_URL}{AJAX_PATH}"))
                .form(&[
                    ("action", "baka_ajax"),
                    ("type", "search_series"),
                    ("q", query.as_str()),
                ])
                .send_text()
                .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
            return Ok(parse_ajax_search(&body));
        }
        let page = vh::page_number(&request);
        let path = vh::filter(&request, "path").unwrap_or("/xem-nhieu-nhat");
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &paged_url(path, page),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        let manga_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &manga_url, DETAILS_FIXTURE);
        let Some(parent_id) = html::attr_after(&body, "post_manga_id", "value") else {
            return Ok(parse_chapter_html(CHAPTERS_HTML_FIXTURE));
        };
        let first = fetch_chapter_page(&parent_id, 1, 16)
            .unwrap_or_else(|| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        let mut html_blocks = first
            .data
            .as_ref()
            .and_then(|data| data.html.clone())
            .into_iter()
            .collect::<Vec<_>>();
        let total = first
            .data
            .as_ref()
            .and_then(|data| data.pagination.as_deref())
            .map(total_pages)
            .unwrap_or(1);
        for page in 2..=total.min(50) {
            if let Some(next) = fetch_chapter_page(&parent_id, page, 16)
                .and_then(|response| response.data)
                .and_then(|data| data.html)
            {
                html_blocks.push(next);
            }
        }
        let chapters = html_blocks
            .iter()
            .flat_map(|block| parse_chapter_html(block))
            .fold(Vec::new(), vh::push_unique_chapter);
        Ok(if chapters.is_empty() {
            parse_chapter_html(CHAPTERS_HTML_FIXTURE)
        } else {
            chapters
        })
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("page-image")
                    || chunk.contains("webtoonContainer")
                    || chunk.contains("src=")
            })
            .filter_map(vh::image_attr)
            .filter(|image| vh::looks_like_image(image))
            .map(|image| vh::absolute_url(BASE_URL, &image))
            .fold(Vec::new(), |mut seen, image| {
                if !seen.contains(&image) {
                    seen.push(image);
                }
                seen
            });
        Ok(if images.is_empty() {
            vec![vh::text_page("Khong tim thay hinh anh")]
        } else {
            vh::image_pages(images, &chapter_url)
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            vh::home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"})),
            )?,
            vh::home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"})),
            )?,
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = vh::normalize_key(BASE_URL, input);
            let is_chapter = key.contains("/chapter-") || key.contains("/chap-");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&key)),
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

fn ajax_client() -> manatan_shared::sdk::http::HttpClient {
    vh::browser_client(BASE_URL).with_referer(format!("{BASE_URL}/"))
}

fn paged_url(path: &str, page: u64) -> String {
    if page > 1 {
        format!("{BASE_URL}{}/page/{page}", path.trim_end_matches('/'))
    } else {
        format!("{BASE_URL}{path}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("comic-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("title=\"Tiếp\""),
    }
}

fn parse_ajax_search(body: &str) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<SearchResponse>(body)
        .or_else(|_| serde_json::from_str(SEARCH_FIXTURE))
        .unwrap_or_default();
    let entries = parsed
        .data
        .and_then(|data| data.series)
        .unwrap_or_default()
        .into_iter()
        .map(|series| {
            let key = vh::normalize_key(BASE_URL, &series.url);
            vh::catalog_item(BASE_URL, key, series.title, series.thumbnail, "adult")
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let manga_id = html::attr_after(body, "post_manga_id", "value");
    let thumbnail = manga_id
        .as_deref()
        .and_then(|id| fetch_chapter_page(id, 1, 1))
        .and_then(|response| response.data)
        .and_then(|data| data.html)
        .and_then(|html| vh::image_attr(&html))
        .or_else(|| html::attr_after(body, "ratingModalCover", "src"))
        .or_else(|| html::attr_after(body, "series-thumbnail", "src"))
        .or_else(|| vh::image_attr(body));
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: thumbnail.map(|image| vh::absolute_url(BASE_URL, &image)),
        authors: icon_span_value(body, "Tác Giả").into_iter().collect(),
        tags: link_texts(body, "genres-tags-container"),
        description: html::text_between(body, "synopsisText", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: vh::status_from_vi(&icon_span_value(body, "Trạng thái").unwrap_or_default()),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapter_page(parent_id: &str, page: u64, per_page: u64) -> Option<ChaptersResponse> {
    let body = ajax_client()
        .post(format!("{BASE_URL}{AJAX_PATH}"))
        .form(&[
            ("action", "baka_ajax"),
            ("type", "load_chapters_paginated"),
            ("parent_id", parent_id),
            ("page", &page.to_string()),
            ("order", "newest_first"),
            ("per_page", &per_page.to_string()),
        ])
        .send_text()
        .ok()?;
    serde_json::from_str(&body).ok()
}

fn parse_chapter_html(body: &str) -> Vec<MangaChapter> {
    body.split("comic-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "text-white", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| vh::parse_vi_date(&value)),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

fn total_pages(html: &str) -> u64 {
    html.split("data-page=\"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next()?.parse::<u64>().ok())
        .max()
        .unwrap_or(1)
}

fn icon_span_value(body: &str, title: &str) -> Option<String> {
    body.find(title)
        .and_then(|index| html::text_between(&body[index..], "<span", "</span>"))
        .map(|value| {
            html::strip_tags(&value)
                .replace(title, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|index| {
            body[index..]
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    data: Option<SearchData>,
}

#[derive(Default, Deserialize)]
struct SearchData {
    series: Option<Vec<SearchSeries>>,
}

#[derive(Default, Deserialize)]
struct SearchSeries {
    title: String,
    url: String,
    thumbnail: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChaptersResponse {
    data: Option<ChaptersData>,
}

#[derive(Default, Deserialize)]
struct ChaptersData {
    html: Option<String>,
    pagination: Option<String>,
}

const LIST_FIXTURE: &str = r#"<main><div class="comic-card"><a href="/truyen/sample" title="Sample"><img src="/cover.jpg"><h3>Sample</h3></a></div></main>"#;
const SEARCH_FIXTURE: &str = r#"{"success":true,"data":{"series":[{"title":"Sample","url":"https://truyentvn.net/truyen/sample","thumbnail":"https://truyentvn.net/cover.jpg"}]}}"#;
const DETAILS_FIXTURE: &str = r#"
<input id="post_manga_id" value="1"><h1>Sample</h1><div id="series-thumbnail"><img src="/cover.jpg"></div><span><i title="Tác Giả"></i><span>Author</span></span><div id="genres-tags-container"><a>Action</a></div><span><i title="Trạng thái"></i><span>Đang tiến hành</span></span><div id="synopsisText">Summary</div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":{"html":"<div class=\"comic-card\"><a href=\"/truyen/sample/chapter-1\" title=\"Chapter 1\"><h3>Chapter 1</h3><span class=\"text-white\">01/01/2024</span></a></div>","pagination":""}}"#;
const CHAPTERS_HTML_FIXTURE: &str = r#"<div class="comic-card"><a href="/truyen/sample/chapter-1" title="Chapter 1"><h3>Chapter 1</h3><span class="text-white">01/01/2024</span></a></div>"#;
const PAGES_FIXTURE: &str =
    r#"<main class="webtoon-mode"><img class="page-image" src="/page1.jpg"></main>"#;

export_manga_source!(SOURCE);
