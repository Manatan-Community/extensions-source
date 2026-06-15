use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: ThienThaiTruyen = ThienThaiTruyen;
const BASE_URL: &str = "https://thienthaitruyen8.com";

struct ThienThaiTruyen;

impl MangaSource for ThienThaiTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "rating"
        } else {
            "latest"
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &browse_url(page, None, None, "all", sort),
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
        let target = browse_url(
            page,
            (!query.is_empty()).then_some(query.as_str()),
            vh::filter(&request, "genre"),
            vh::filter(&request, "status").unwrap_or("all"),
            vh::filter(&request, "sort").unwrap_or("latest"),
        );
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
            .unwrap_or_else(|| "/truyen-tranh/sample/chap-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let images = images_from_html(&vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE));
        Ok(pages_or_text(images, &chapter_url))
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

fn browse_url(
    page: u64,
    query: Option<&str>,
    genre: Option<&str>,
    status: &str,
    sort: &str,
) -> String {
    let mut pairs = vec![
        format!("sort={}", url::query_escape(sort)),
        format!("status={}", url::query_escape(status)),
        format!("page={page}"),
    ];
    if let Some(query) = query {
        pairs.push(format!("name={}", url::query_escape(query)));
    }
    if let Some(genre) = genre {
        pairs.push(format!("genres={}", url::query_escape(genre)));
    }
    format!("{BASE_URL}/tim-kiem-nang-cao?{}", pairs.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("/truyen-tranh/")
                && (chunk.contains("line-clamp-2") || chunk.contains("<img"))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "line-clamp-2", "</span>")
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
        has_next_page: vh::has_next(body) || body.contains("Sau"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = body.split("comic-content").next().unwrap_or(body);
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Manga".into()),
        cover: html::attr_after(body, "alt=\"poster\"", "src")
            .or_else(|| vh::image_attr(body))
            .map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(info, "Tác giả").into_iter().collect(),
        tags: body
            .split("/the-loai/")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "comic-content", "</p>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: info_value(info, "Trạng thái")
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
                .trim_matches(':')
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(vh::title_from(chunk).unwrap_or_else(|| "Chapter".into())),
                date_uploaded: html::text_between(chunk, "<span", "</span>")
                    .and_then(|v| vh::parse_dd_mm_yyyy(&html::strip_tags(&v))),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn images_from_html(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("title=\"banner\""))
        .filter_map(|chunk| vh::image_attr(chunk).map(|v| vh::absolute_url(BASE_URL, &v)))
        .collect()
}

fn pages_or_text(images: Vec<String>, referer: &str) -> Vec<MangaPage> {
    if images.is_empty() {
        vec![vh::text_page("Khong tim thay hinh anh")]
    } else {
        images
            .iter()
            .enumerate()
            .map(|(i, image)| vh::image_page(i, image, referer))
            .collect()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a href="/truyen-tranh/sample"><span class="line-clamp-2">Sample</span><img src="/cover.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><img alt="poster" src="/cover.jpg"><p class="comic-content">Summary</p><div class="chapter-items"><a href="/truyen-tranh/sample/chap-1">Chapter 1</a></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="center"><img src="/page1.jpg"></div>"#;
