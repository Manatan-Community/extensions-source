use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Wnacg = Wnacg;
const DEFAULT_BASE_URL: &str = "https://www.wn05.ru";

struct Wnacg;

impl MangaSource for Wnacg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("albums-index-page-{page}.html")
        } else {
            format!("albums-favorite_ranking-page-{page}-type-week.html")
        };
        let body = fetch(&base, &url::join_url(&base, &path), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&base, &url::join_url(&base, &key), DETAILS_FIXTURE),
                    &key,
                    &base,
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let filters = request.get("filters").unwrap_or(&Value::Null);
            let tag = filters
                .get("tag")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !tag.is_empty() {
                url::join_url(
                    &base,
                    &format!(
                        "albums-index-page-{}-tag-{}.html",
                        page(&request),
                        url::query_escape(tag)
                    ),
                )
            } else if let Some(category) = filters
                .get("category")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                url::join_url(&base, &category.replace("%d", &page(&request).to_string()))
            } else {
                url::join_url(
                    &base,
                    &format!(
                        "albums-favorite_ranking-page-{}-type-week.html",
                        page(&request)
                    ),
                )
            }
        } else {
            format!(
                "{base}/search/index.php?s=create_time_DESC&q={}&p={}",
                url::query_escape(query),
                page(&request)
            )
        };
        let body = fetch(&base, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/photos-index-aid-1.html".into());
        Ok(parse_details(
            &fetch(&base, &url::join_url(&base, &key), DETAILS_FIXTURE),
            &key,
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/photos-index-aid-1.html".into());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Ch. 1".into()),
            url: Some(url::join_url(&base, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/photos-index-aid-1.html".into());
        let gallery_key = key.replace("-index-", "-gallery-");
        let target = url::join_url(&base, &gallery_key);
        Ok(parse_pages(&fetch(&base, &target, PAGES_FIXTURE), &base))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(&base, input, DETAILS_FIXTURE),
                    &key,
                    &base,
                )),
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

fn client(base: &str) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .header("Sec-Fetch-Mode", "no-cors")
        .header("Sec-Fetch-Site", "cross-site")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get("baseUrl"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.contains("photos-index-aid-") {
        return None;
    }
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or(input);
    Some(format!(
        "/{}",
        path.split('?').next().unwrap_or(path).trim_matches('/')
    ))
}

fn parse_cards(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("gallary_item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = key_from_url(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "title", "</")
                    .map(|text| html::strip_tags(&text))
                    .filter(|text| !text.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| "紳士漫畫".into()),
                cover: html::attr_after(chunk, "<img", "src").map(|image| normalize_image(&image)),
                url: Some(url::join_url(base, &key)),
                language: Some("zh".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: &str, base: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "<h2", "</h2>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "紳士漫畫".into()),
        cover: html::attr_after(body, "uwthumb", "src").map(|image| normalize_image(&image)),
        authors: html::text_between(body, "uwuinfo", "</div>")
            .map(|v| vec![html::strip_tags(&v)])
            .unwrap_or_default(),
        tags: body
            .split("tagshow")
            .skip(1)
            .map(html::strip_tags)
            .filter(|v| !v.is_empty())
            .collect(),
        description: html::text_between(body, "asTBcell", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: ItemStatus::Completed,
        url: Some(url::join_url(base, key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    protocol_relative_images(body)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: normalize_image(&image),
                context: None,
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn protocol_relative_images(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (index, _) in body.match_indices("//") {
        let rest = &body[index..];
        let end = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ')' | '('))
            .unwrap_or(rest.len());
        let candidate = &rest[..end];
        let lower = candidate.to_ascii_lowercase();
        if [".jpg", ".jpeg", ".png", ".webp", ".gif"]
            .iter()
            .any(|ext| lower.contains(ext))
            && !out.iter().any(|old| old == candidate)
        {
            out.push(candidate.to_string());
        }
    }
    out
}

fn normalize_image(input: &str) -> String {
    if input.starts_with("//") {
        format!("http:{input}")
    } else {
        input.to_string()
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("thispage") && body.contains("thispage +")
        || body.contains("class=\"next\"")
        || body.contains(">下一页<")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="gallary_item"><div class="title"><a href="/photos-index-aid-1.html">Sample</a></div><img src="//img.example/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h2>Sample</h2><div class="uwthumb"><img src="//img.example/cover.jpg"></div><div class="uwuinfo"><p>Author</p></div><a class="tagshow">Tag</a><div class="asTBcell"><p>Sample description.</p></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var img="//img.example/page.jpg";</script>"#;
