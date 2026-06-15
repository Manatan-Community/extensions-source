use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde_json::{Value, json};

const SOURCE: TuiTruyen = TuiTruyen;
const BASE_URL: &str = "https://tuitruyen.top";

struct TuiTruyen;

impl MangaSource for TuiTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(Paged {
                entries: parse_popular(&vh::fetch_document(BASE_URL, BASE_URL, POPULAR_FIXTURE)),
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let target = format!("{BASE_URL}/manga?page={page}");
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/manga/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let mut pairs = vec![format!("page={page}")];
        if !query.is_empty() {
            pairs.push(format!("q={}", url::query_escape(&query)));
        }
        if let Some(status) = vh::filter(&request, "status") {
            pairs.push(format!("status={}", url::query_escape(status)));
        }
        for value in vh::selected_array(&request, "include") {
            let id = value.split(':').next().unwrap_or(&value);
            pairs.push(format!("include={}", url::query_escape(id)));
        }
        let target = format!("{BASE_URL}/manga?{}", pairs.join("&"));
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let mut current_url = vh::absolute_url(BASE_URL, &key);
        let mut visited = Vec::new();
        let mut chapters = Vec::new();
        for _ in 0..25 {
            if visited.contains(&current_url) {
                break;
            }
            visited.push(current_url.clone());
            let body = vh::fetch_document(BASE_URL, &current_url, DETAILS_FIXTURE);
            chapters = parse_chapters(&body)
                .into_iter()
                .fold(chapters, vh::push_unique_chapter);
            let Some(next) = next_chapter_page(&body) else {
                break;
            };
            current_url = vh::absolute_url(BASE_URL, &next);
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| chunk.contains("page-media"))
            .filter(|chunk| !chunk.contains("<noscript"))
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
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/manga/") {
            let is_chapter = key.matches('/').count() > 2;
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

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("homepage-ranking-item__link")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::attr_after(chunk, "homepage-ranking-item__title", "title")
                .or_else(|| html::text_between(chunk, "homepage-ranking-item__title", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk),
                "safe",
            ))
        })
        .fold(Vec::new(), vh::push_unique)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-card--list")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::attr_after(chunk, "<h3", "title")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| {
                    html::attr_after(chunk, "<img", "alt")
                        .map(|alt| alt.trim_start_matches("Bìa ").to_string())
                })
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
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
        has_next_page: body.contains("Trang sau") && !body.contains("is-disabled"),
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
        title: html::text_between(body, "manga-detail-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "detail-cover", "src")
            .or_else(|| vh::image_attr(body))
            .map(|image| vh::absolute_url(BASE_URL, &image)),
        authors: meta_links(body, "Tác giả"),
        tags: link_texts(body, "manga-detail-genre-chips"),
        description: html::text_between(body, "data-description-content", "</")
            .or_else(|| html::text_between(body, "manga-description__text", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: vh::status_from_vi(
            &html::text_between(body, "manga-status-pill", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-link")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "chapter-num", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let date_text = html::attr_after(chunk, "chapter-time", "title")
                .or_else(|| {
                    html::text_between(chunk, "chapter-time", "</")
                        .map(|value| html::strip_tags(&value))
                })
                .unwrap_or_default();
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: vh::parse_vi_date(date_text.trim_start_matches("Cập nhật").trim()),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

fn next_chapter_page(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|chunk| {
            chunk.contains("Trang chương sau")
                && !chunk.contains("is-disabled")
                && !html::attr(chunk, "href").is_some_and(|href| href == "#")
        })
        .and_then(|chunk| html::attr(chunk, "href"))
}

fn meta_links(body: &str, label: &str) -> Vec<String> {
    body.split("manga-detail-meta-line")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
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

const POPULAR_FIXTURE: &str = r#"
<ol class="homepage-ranking-list" data-ranking-period="total"><a class="homepage-ranking-item__link" href="/manga/sample"><span class="homepage-ranking-item__title" title="Sample">Sample</span><img src="/cover.jpg"></a></ol>
"#;
const LIST_FIXTURE: &str = r#"
<article class="manga-card--list"><a href="/manga/sample"><h3 title="Sample">Sample</h3><img src="/cover.jpg"></a></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="manga-detail-title">Sample</h1><div class="detail-cover"><img src="/cover.jpg"></div><p class="manga-detail-meta-line"><span class="manga-detail-meta-label">Tác giả</span><a class="inline-link">Author</a></p><div class="manga-detail-genre-chips"><a class="chip">Action</a></div><span class="manga-status-pill">Còn tiếp</span><div data-description-content>Summary</div><ul class="chapter-list"><li class="chapter"><a class="chapter-link" href="/manga/sample/chapter-1"><span class="chapter-num">Chapter 1</span><span class="chapter-time" title="Cập nhật 01/01/2024">01/01/2024</span></a></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<img class="page-media" data-src="/page1.jpg">"#;

export_manga_source!(SOURCE);
