use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TruyenQQ = TruyenQQ;
const DEFAULT_BASE_URL: &str = "https://truyenqqko.com";

struct TruyenQQ;

impl MangaSource for TruyenQQ {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "truyen-moi-cap-nhat"
        } else {
            "truyen-yeu-thich"
        };
        let target = if page > 1 {
            format!("{base}/{path}/trang-{page}")
        } else {
            format!("{base}/{path}")
        };
        Ok(parse_listing(
            &base,
            &vh::fetch_document(&base, &target, LIST_FIXTURE),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(&base, &query, "/truyen-tranh/") {
            return Ok(Paged {
                entries: vec![details_by_key(&base, &key)],
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let target = if !query.is_empty() {
            let page_path = if page > 1 {
                format!("/trang-{page}")
            } else {
                String::new()
            };
            format!("{base}/tim-kiem{page_path}?q={}", url::query_escape(&query))
        } else {
            advanced_search_url(&base, page, &request)
        };
        Ok(parse_listing(
            &base,
            &vh::fetch_document(&base, &target, LIST_FIXTURE),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(
            &base,
            &vh::fetch_document(&base, &vh::absolute_url(&base, &key), DETAILS_FIXTURE),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(&base, &key);
        let body = vh::fetch_document(&base, &chapter_url, PAGES_FIXTURE);
        let images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| chunk.contains("page-chapter") || !chunk.contains("stress.gif"))
            .filter_map(vh::image_attr)
            .filter(|image| vh::looks_like_image(image))
            .map(|image| vh::absolute_url(&base, &image))
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

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            vh::home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, "popular")),
            )?,
            vh::home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, "latest")),
            )?,
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| vh::absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| vh::absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = vh::key_from_url(&base, input, "/truyen-tranh/") {
            let is_chapter = key.contains("/chapter-") || key.contains("/chap-");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&base, &key)),
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if !next.is_object() {
        next = json!({});
    }
    next["page"] = json!(1);
    next["listingId"] = json!(listing);
    next
}

fn advanced_search_url(base: &str, page: u64, request: &Value) -> String {
    let page_path = if page > 1 {
        format!("/trang-{page}")
    } else {
        String::new()
    };
    let mut pairs = Vec::new();
    for id in [
        "country",
        "status",
        "minchapter",
        "sort",
        "category",
        "notcategory",
    ] {
        if let Some(value) = vh::filter(request, id) {
            pairs.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    if pairs.is_empty() {
        format!("{base}/tim-kiem-nang-cao{page_path}")
    } else {
        format!("{base}/tim-kiem-nang-cao{page_path}?{}", pairs.join("&"))
    }
}

fn parse_listing(base: &str, body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("book_") || chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = vh::normalize_key(base, &href);
            let title = html::text_between(chunk, "qtip", "</a>")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| vh::title_from(chunk))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                base,
                key,
                title,
                vh::image_attr(chunk),
                "safe",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page_redirect") && body.contains("not(.active)")
            || vh::has_next(body),
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(
        base,
        &vh::fetch_document(base, &vh::absolute_url(base, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(base: &str, body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: vh::normalize_key(base, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "itemprop=image", "src")
            .or_else(|| vh::image_attr(body))
            .map(|image| vh::absolute_url(base, &image)),
        authors: link_texts(body, "org"),
        tags: link_texts(body, "list01"),
        description: html::text_between(body, "story-detail-info", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: vh::status_from_vi(&html::strip_tags(
            &html::text_between(body, "status", "</li>").unwrap_or_default(),
        )),
        url: Some(vh::absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(base: &str, body: &str) -> Vec<MangaChapter> {
    body.split("works-chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(base, &href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "time-chap", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| vh::parse_vi_date(&value)),
                url: Some(vh::absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
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
<ul class="grid"><li><div class="book_info"><div class="qtip"><a href="/truyen-tranh/sample">Sample</a></div></div><div class="book_avatar"><img src="/cover.jpg"></div></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><img itemprop="image" src="/cover.jpg"><div class="list-info"><p class="org"><a>Author</a></p><div class="status"><p>Đang Cập Nhật</p></div></div><ul class="list01"><li><a>Action</a></li></ul><div class="story-detail-info"><p>Summary</p></div><div class="works-chapter-list"><div class="works-chapter-item"><a href="/truyen-tranh/sample/chapter-1">Chapter 1</a><span class="time-chap">01/01/2024</span></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="page-chapter"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
