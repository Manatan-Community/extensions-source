use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: Tranh18 = Tranh18;
const BASE_URL: &str = "https://tranh18.cc";

struct Tranh18;

impl MangaSource for Tranh18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let target =
            if request.get("listingId").and_then(Value::as_str) == Some("latest") && page > 1 {
                format!("{BASE_URL}/update?page={page}")
            } else if request.get("listingId").and_then(Value::as_str) == Some("latest") {
                format!("{BASE_URL}/update")
            } else {
                BASE_URL.to_string()
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
        let target = if query.is_empty() {
            let mut pairs = vec![format!("page={page}")];
            for id in ["tag", "end", "area"] {
                if let Some(value) = vh::filter(&request, id) {
                    pairs.push(format!("{id}={}", url::query_escape(value)));
                }
            }
            format!("{BASE_URL}/comics?{}", pairs.join("&"))
        } else {
            format!(
                "{BASE_URL}/search?keyword={}&page={page}",
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = vh::title_from(chunk)
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            let cover = html::attr_after(chunk, "mh-cover", "style")
                .and_then(|style| vh::image_from_style(&style))
                .or_else(|| vh::image_attr(chunk));
            Some(vh::catalog_item(BASE_URL, key, title, cover, "adult"))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: vh::has_next(body) || body.contains("page-pagination"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = info_value(body, "Trạng thái").unwrap_or_default();
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Manga".into()),
        cover: vh::image_attr(body).map(|v| vh::absolute_url(BASE_URL, &v)),
        authors: info_value(body, "Tác giả").into_iter().collect(),
        tags: body
            .split("Từ khóa")
            .nth(1)
            .unwrap_or(body)
            .split("<a")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v)))
            .collect(),
        description: html::text_between(body, "content", "</p>")
            .or_else(|| html::text_between(body, "detail-desc", "</p>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: vh::status_from_vi(&status_text),
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
                .replace('：', "")
                .replace(':', "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("detail-list-select")
        .nth(1)
        .unwrap_or(body)
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = vh::title_from(chunk).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
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

const LIST_FIXTURE: &str = r#"<li><div class="mh-item"><a href="/sample" title="Sample"><p class="mh-cover" style="background-image:url(/cover.jpg)"></p></a></div></li>"#;
const DETAILS_FIXTURE: &str = r#"<div class="info"><h1>Sample</h1></div><ul class="detail-list-select"><li><a href="/sample/chapter-1">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<img class="lazy" data-original="/page1.jpg">"#;
