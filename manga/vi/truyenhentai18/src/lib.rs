use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TruyenHentai18 = TruyenHentai18;
const BASE_URL: &str = "https://truyenhentai18.net";

struct TruyenHentai18;

impl MangaSource for TruyenHentai18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "/xem-nhieu-nhat"
        } else {
            "/moi-cap-nhat"
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &paged(path, page),
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
        let page = vh::page_number(&request);
        let target = if query.is_empty() {
            vh::filter(&request, "genre")
                .map(|g| paged(&format!("/category/{g}"), page))
                .unwrap_or_else(|| paged("/xem-nhieu-nhat", page))
        } else {
            format!("{BASE_URL}?s={}", url::query_escape(&query))
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&vh::fetch_document(
            BASE_URL,
            &vh::absolute_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let images = parse_images(&vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE));
        Ok(if images.is_empty() {
            vec![vh::text_page("Khong tim thay hinh anh")]
        } else {
            images
                .iter()
                .enumerate()
                .map(|(i, image)| vh::image_page(i, image, &chapter_url))
                .collect()
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

fn paged(path: &str, page: u64) -> String {
    if page > 1 {
        format!("{BASE_URL}{path}/page/{page}")
    } else {
        format!("{BASE_URL}{path}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("col-6")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .map(|v| html::strip_tags(&v))
                .or_else(|| vh::title_from(chunk))
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
        has_next_page: vh::has_next(body),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Manga".into()),
        cover: vh::image_attr(body).map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(body, "Tác giả:").into_iter().collect(),
        tags: body
            .split("badge bg-primary")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "description", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: info_value(body, "Trạng thái:")
            .map(|v| vh::status_from_vi(&v))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .map(|idx| {
            html::strip_tags(body[idx..].split("</").next().unwrap_or_default())
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(vh::title_from(chunk).unwrap_or_else(|| "Chapter".into())),
                date_uploaded: html::text_between(chunk, "chapter-date", "</")
                    .and_then(|v| vh::parse_dd_mm_yyyy(&html::strip_tags(&v))),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_images(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| vh::image_attr(chunk).map(|v| vh::absolute_url(BASE_URL, &v)))
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="col-6 col-md-4"><a href="/sample"><h2>Sample</h2><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><img class="manga-cover" src="/cover.jpg"><div class="chapter-item"><a class="fw-bold" href="/sample/chapter-1">Chapter 1</a></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="viewer" class="chapter-container"><img src="/page1.jpg"></div>"#;
