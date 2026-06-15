use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: Truyen18 = Truyen18;
const BASE_URL: &str = "https://truyen18.co";

struct Truyen18;

impl MangaSource for Truyen18 {
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
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/doc-truyen") {
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
        } else if page > 1 {
            format!(
                "{BASE_URL}/search/page/{page}?q={}",
                url::query_escape(&query)
            )
        } else {
            format!("{BASE_URL}/search?q={}", url::query_escape(&query))
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/doc-truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/doc-truyen/sample".into());
        Ok(parse_chapters(&vh::fetch_document(
            BASE_URL,
            &vh::absolute_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/doc-truyen/sample/chap-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        let images = parse_images(&body, slug);
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
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/doc-truyen") {
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
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/doc-truyen") && chunk.contains("<h3"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|v| html::strip_tags(&v))
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
        has_next_page: body.contains("rel=\"next\""),
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
        title: html::text_between(body, "main", "</h1>")
            .and_then(|v| html::text_between(&v, "<h1", "</h1>").or(Some(v)))
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Manga".into()),
        cover: vh::image_attr(body).map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(body, "Tác giả").into_iter().collect(),
        tags: body
            .split("/category/")
            .chain(body.split("/tag/"))
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "max-h-96", "</p>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: info_value(body, "Trạng thái")
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
            html::strip_tags(body[idx..].split("</div>").next().unwrap_or_default())
                .replace(label, "")
                .replace(':', "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("Danh sách chương")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/doc-truyen") {
                return None;
            }
            let key = vh::normalize_key(BASE_URL, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "truncate", "</")
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

fn parse_images(body: &str, slug: &str) -> Vec<String> {
    let mut images = Vec::new();
    if let Some(token) = current_chapter_token(body, slug) {
        if let Some(escaped) = escaped_content_for_token(body, &token) {
            images.extend(image_srcs(&decode_escaped(&escaped)));
        }
    }
    if images.is_empty() {
        images.extend(image_srcs(body).into_iter().filter(|image| {
            slug.is_empty() || image.contains(slug) || !image.contains("/doc-truyen/")
        }));
    }
    images
}

fn current_chapter_token(body: &str, slug: &str) -> Option<String> {
    let idx = body.find(slug)?;
    let before = &body[..idx];
    before.rsplit('"').nth(1).map(ToString::to_string)
}

fn escaped_content_for_token(body: &str, token: &str) -> Option<String> {
    let idx = body.find(token)?;
    let after = &body[idx + token.len()..];
    after.split('"').nth(1).map(ToString::to_string)
}

fn decode_escaped(content: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{content}\"")).unwrap_or_else(|_| {
        content
            .replace("\\u003c", "<")
            .replace("\\u003e", ">")
            .replace("\\u0026", "&")
            .replace("\\u002F", "/")
            .replace("\\\"", "\"")
            .replace("\\/", "/")
            .replace("\\n", "\n")
    })
}

fn image_srcs(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| vh::image_attr(chunk).map(|v| vh::absolute_url(BASE_URL, &v)))
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<main><div class="grid"><a href="/doc-truyen/sample"><h3>Sample</h3><img src="/cover.jpg"></a></div></main>"#;
const DETAILS_FIXTURE: &str = r#"<main><h1>Sample</h1><img src="/cover.jpg"><section><h2>Danh sách chương</h2><a href="/doc-truyen/sample/chap-1"><span class="truncate">Chapter 1</span></a></section></main>"#;
const PAGES_FIXTURE: &str = r#"<img src="/page1.jpg">"#;
