use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TruyenHentaivn = TruyenHentaivn;
const BASE_URL: &str = "https://truyenhentaivn.club";

struct TruyenHentaivn;

impl MangaSource for TruyenHentaivn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "/top-de-cu"
        } else {
            "/danh-sach"
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &list_url(path, page),
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
                .map(|path| list_url(path, page))
                .unwrap_or_else(|| list_url("/danh-sach", page))
        } else {
            format!(
                "{BASE_URL}/tim-kiem-truyen/?q={}&page={page}",
                url::query_escape(&query)
            )
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

fn list_url(path: &str, page: u64) -> String {
    format!(
        "{BASE_URL}{}?page={page}",
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        }
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("entry text-center")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "name", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::attr_after(chunk, "name", "title")
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
        has_next_page: vh::has_next(body) || body.contains("z-pagination") && body.contains("Next"),
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
        authors: html::text_between(body, "author", "</")
            .map(|v| html::strip_tags(&v))
            .into_iter()
            .collect(),
        tags: body
            .split("meta-data")
            .nth(1)
            .unwrap_or(body)
            .split("/the-loai")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "comic-description", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: html::text_between(body, "Tình trạng", "</")
            .map(|v| vh::status_from_vi(&html::strip_tags(&v)))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chap-list")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "span class=\"name\"", "</span>")
                        .map(|v| html::strip_tags(&v))
                        .or_else(|| vh::title_from(chunk))
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                date_uploaded: vh::parse_dd_mm_yyyy(chunk),
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

const LIST_FIXTURE: &str = r#"<div class="entry text-center"><a class="name" href="/sample" title="Sample">Sample</a><a class="s-thumb"><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="comic-info"><div class="info"><h1 class="name">Sample</h1></div><div class="book"><img src="/cover.jpg"></div></div><div class="chap-list"><a class="d-flex justify-content-between" href="/sample/chapter-1"><span class="name">Chapter 1</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter-content"><img src="/page1.jpg"></div>"#;
