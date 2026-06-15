use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TruyenGG = TruyenGG;
const BASE_URL: &str = "https://foxtruyen2.com";

struct TruyenGG;

impl MangaSource for TruyenGG {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let base_path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "/top-binh-chon"
        } else {
            "/truyen-moi-cap-nhat"
        };
        let target = if page > 1 {
            format!("{BASE_URL}{base_path}/trang-{page}.html")
        } else {
            format!("{BASE_URL}{base_path}")
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
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
        let endpoint = if query.is_empty() {
            "tim-kiem-nang-cao"
        } else {
            "tim-kiem"
        };
        let mut target = if page > 1 {
            format!("{BASE_URL}/{endpoint}/trang-{page}.html")
        } else {
            format!("{BASE_URL}/{endpoint}")
        };
        let mut pairs = Vec::new();
        if !query.is_empty() {
            pairs.push(format!("q={}", url::query_escape(&query)));
        } else {
            for id in ["country", "status", "category"] {
                if let Some(value) = vh::filter(&request, id) {
                    pairs.push(format!("{id}={}", url::query_escape(value)));
                }
            }
        }
        if !pairs.is_empty() {
            target.push('?');
            target.push_str(&pairs.join("&"));
        }
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("item_home")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "book_name", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "book_name", "</a>")
                .map(|v| html::strip_tags(&v))
                .or_else(|| vh::title_from(chunk))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk),
                "safe",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: vh::has_next(body) || body.contains("pagination") && body.contains("active"),
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
        title: html::text_between(body, "itemprop=name", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Manga".into()),
        cover: vh::image_attr(body).map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(body, "Tác Giả").into_iter().collect(),
        tags: body
            .split("fx-genres")
            .nth(1)
            .unwrap_or(body)
            .split("<a")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "fx-synopsis", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: html::text_between(body, "fx-status", "</")
            .map(|v| vh::status_from_vi(&html::strip_tags(&v)))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .map(|idx| {
            html::strip_tags(body[idx..].split("</span>").next().unwrap_or_default())
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("fx-chap-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(vh::title_from(chunk).unwrap_or_else(|| "Chapter".into())),
                date_uploaded: vh::parse_dd_mm_yyyy(chunk),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_images(body: &str) -> Vec<String> {
    body.split("content_detail")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| vh::image_attr(chunk).map(|v| vh::absolute_url(BASE_URL, &v)))
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="item_home"><a class="book_name" href="/sample">Sample</a><div class="image-cover"><img data-src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 itemprop="name">Sample</h1><div class="fx-cover"><img src="/cover.jpg"></div><ul class="fx-chap-list"><li class="fx-chap-item"><a href="/sample/chapter-1">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div class="content_detail"><img src="/page1.jpg"></div>"#;
