use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TruyenTranh3Q = TruyenTranh3Q;
const BASE_URL: &str = "https://manhua3q.com";

struct TruyenTranh3Q;

impl MangaSource for TruyenTranh3Q {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "truyen-moi-cap-nhat"
        } else {
            "truyen-yeu-thich"
        };
        let target = format!("{BASE_URL}/danh-sach/{path}?page={page}");
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/truyen-tranh/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let target = advanced_search_url(page, &query, &request);
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&vh::fetch_document(
            BASE_URL,
            &vh::absolute_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| chunk.contains("page-chapter") || chunk.contains("data-src"))
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
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/truyen-tranh/") {
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

fn advanced_search_url(page: u64, query: &str, request: &Value) -> String {
    let mut pairs = vec![format!("page={page}")];
    if !query.is_empty() {
        pairs.push(format!("keyword={}", url::query_escape(query)));
    }
    for id in [
        "sort",
        "status",
        "country",
        "minChap",
        "categories",
        "nocategories",
    ] {
        if let Some(value) = vh::filter(request, id) {
            pairs.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    format!("{BASE_URL}/tim-kiem-nang-cao?{}", pairs.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| vh::title_from(chunk))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            let cover = vh::image_attr(chunk).map(|image| {
                if let Some(encoded) = image.split("url=").nth(1) {
                    encoded.to_string()
                } else {
                    image
                }
            });
            Some(vh::catalog_item(BASE_URL, key, title, cover, "adult"))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page_redirect") || vh::has_next(body),
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
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "book_avatar", "src")
            .or_else(|| vh::image_attr(body))
            .map(|image| vh::absolute_url(BASE_URL, &image)),
        authors: info_values(body, "author"),
        tags: link_texts(body, "list01"),
        description: html::text_between(body, "story-detail-info", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: vh::status_from_vi(&info_values(body, "status").join(" ")),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("works-chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "name-chap", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "time-chap", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| vh::parse_vi_date(&value)),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

fn info_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, "col-xs-9", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
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

const LIST_FIXTURE: &str = r#"
<ul class="list_grid grid"><li><div class="book_avatar"><a><img src="/cover.jpg"></a></div><h3><a href="/truyen-tranh/sample">Sample</a></h3></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="book_info"><div class="book_other"><h1 itemprop="name">Sample</h1><ul class="list-info"><li class="author"><p class="col-xs-9">Author</p></li><li class="status"><p class="col-xs-9">Đang Cập Nhật</p></li></ul><ul class="list01"><li><a>Action</a></li></ul></div><div class="book_avatar"><img src="/cover.jpg"></div></div><div class="book_detail"><div class="story-detail-info">Summary</div></div><div class="works-chapter-list"><div class="works-chapter-item"><span class="name-chap"><a href="/truyen-tranh/sample/chapter-1">Chapter 1</a></span><span class="time-chap">01/01/2024</span></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter_content"><div class="page-chapter"><img data-src="/page1.jpg"></div></div>"#;

export_manga_source!(SOURCE);
