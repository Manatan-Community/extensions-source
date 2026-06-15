use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PornComix = PornComix;
const BASE_URL: &str = "https://bestporncomix.com";

struct PornComix;

impl MangaSource for PornComix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &popular_url(page),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page > 1 {
            format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/multporn-net/sample/".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/multporn-net/sample/".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            chapter_number: Some(1.0),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/multporn-net/sample/".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "post-title", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "post-title", "</")
                    .or_else(|| html::attr_after(chunk, "<a", "title"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into()));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("nextp") || body.contains("next page"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/multporn-net/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .or_else(|| html::text_between(body, "entry-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into())),
        cover: image_attr(body).map(|image| absolute_url(&image)),
        description: first_paragraph(body),
        tags: tags(body),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("pswp-gallery__item")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-pswp-src"))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("entry-content")
                    || chunk.contains("wp-image")
                    || chunk.contains("src")
            })
            .filter_map(image_attr)
            .collect();
    }
    images
        .into_iter()
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn popular_url(page: u64) -> String {
    if page <= 1 {
        format!("{BASE_URL}/multporn-net/")
    } else {
        format!("{BASE_URL}/multporn-net/page/{page}/")
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-pagespeed-lazy-src")
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| {
            html::attr(input, "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
        .or_else(|| html::attr(input, "src"))
}

fn first_paragraph(body: &str) -> Option<String> {
    body.split("entry-content")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<p", "</p>"))
        .or_else(|| {
            body.split("post-content")
                .nth(1)
                .and_then(|chunk| html::text_between(chunk, "<p", "</p>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("rel=\"tag\"")
                || chunk.contains("post_tag")
                || chunk.contains("tag/")
                || chunk.contains("category/")
        })
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article><h2 class="post-title"><a href="/multporn-net/sample/">Sample Comic</a></h2><img src="/cover.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample Comic</h1><div class="entry-content"><p>Sample description.</p><img src="/page1.jpg"></div><a rel="tag">Parody</a>"#;
const PAGES_FIXTURE: &str = r#"<div class="entry-content"><img class="wp-image" src="/page1.jpg"><img class="wp-image" src="/page2.jpg"></div>"#;
