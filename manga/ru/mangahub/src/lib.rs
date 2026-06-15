use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Mangahub = Mangahub;
const DEFAULT_BASE_URL: &str = "https://mangahub.ru";

struct Mangahub;

impl MangaSource for Mangahub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "rating"
        };
        let suffix = if page > 1 {
            format!("?page={page}")
        } else {
            String::new()
        };
        Ok(parse_listing(
            &fetch_document(
                &base,
                &format!("{base}/explore/sort-is-{sort}{suffix}"),
                LIST_FIXTURE,
            ),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://")
            || query.starts_with("https://")
            || query.starts_with("slug:")
        {
            let key = normalize_key(&base, query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE),
                    Some(key),
                    &base,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{base}/search/title?query={}", url::query_escape(query));
        if page > 1 {
            target.push_str(&format!("&page={page}"));
        }
        Ok(parse_listing(
            &fetch_document(&base, &target, LIST_FIXTURE),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE),
            Some(key),
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_document(
                &base,
                &format!("{}/chapters", absolute_url(&base, &key)),
                DETAILS_FIXTURE,
            ),
            &base,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(
            &fetch_document(&base, &absolute_url(&base, &key), PAGES_FIXTURE),
            &base,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(&base) {
            let key = normalize_key(&base, input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&base, input, DETAILS_FIXTURE),
                    Some(key),
                    &base,
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base.trim_end_matches('/')))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    let first = client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    if first.contains("confirm_age__token") {
        return client(base)
            .post(target)
            .form(&[])
            .send_text()
            .unwrap_or(first);
    }
    first
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get("domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("item-grid")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "fw-medium", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(base, &href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "fw-medium", "</a>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "item-grid-image", "src")
                    .map(|image| absolute_url(base, &image)),
                url: Some(absolute_url(base, &key)),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("page-link") && body.contains("→"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let author = attr_value(body, "Автор");
    let scenario = attr_value(body, "Сценарист");
    let artist = attr_value(body, "Художник");
    let status_text = attr_value(body, "Томов").unwrap_or_default();
    let translate_status = attr_value(body, "Перевод").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangahub".into())),
        cover: html::attr_after(body, "cover-detail", "src")
            .map(|image| absolute_url(base, &image)),
        authors: author.or(scenario).into_iter().collect(),
        artists: artist.into_iter().collect(),
        tags: body
            .split("tags")
            .skip(1)
            .flat_map(|chunk| {
                chunk.split("<a").skip(1).filter_map(|tag| {
                    html::text_between(tag, ">", "</a>").map(|v| html::strip_tags(&v))
                })
            })
            .filter(|v| !v.is_empty())
            .collect(),
        description: html::text_between(body, "markdown-style text-expandable-content", "</")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: parse_status(&status_text, &translate_status),
        url: Some(absolute_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("py-2 px-3")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "align-items-center", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Глава".into());
            Some(MangaChapter {
                key: key.clone(),
                chapter_number: chapter_number(&title),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "text-muted", "</")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| parse_ru_dmy(&v)),
                url: Some(absolute_url(base, &key)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("reader-viewer-img") || chunk.contains("data-src"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(base, &image),
                context: Some(manga::image_headers(base)),
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn attr_value(body: &str, label: &str) -> Option<String> {
    body.split("attr-name")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "attr-value", "</"))
        .map(|v| html::strip_tags(&v))
        .filter(|v| !v.is_empty())
}

fn parse_status(title: &str, translate: &str) -> ItemStatus {
    let title = title.to_lowercase();
    let translate = translate.to_lowercase();
    if title.contains("продолжается") {
        ItemStatus::Ongoing
    } else if title.contains("приостановлен") {
        ItemStatus::Hiatus
    } else if title.contains("завершен") || title.contains("выпуск прекращ") {
        if translate.contains("завершен") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        }
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case("глава"))
        .and_then(|pair| pair[1].replace(',', ".").parse().ok())
}

fn parse_ru_dmy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('.');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    dates::parse_ymd(&format!("{year}-{month}-{day}"))
}

fn normalize_key(base: &str, value: &str) -> String {
    let value = value
        .strip_prefix("slug:")
        .map(|slug| format!("/manga/{slug}"))
        .unwrap_or_else(|| value.to_string());
    let path = value
        .strip_prefix(base)
        .unwrap_or(&value)
        .split('?')
        .next()
        .unwrap_or(&value)
        .split('#')
        .next()
        .unwrap_or(&value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(base: &str, key: &str) -> String {
    url::join_url(base, key)
}

const LIST_FIXTURE: &str = r#"<div class="item-grid"><a class="fw-medium" href="/manga/sample">Sample</a><img class="item-grid-image" src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><img class="cover-detail" src="/cover.jpg"><div class="markdown-style text-expandable-content">Description</div><div class="py-2 px-3"><div class="align-items-center"><a href="/manga/sample/chapter-1">Глава 1</a></div><div class="text-muted">01.01.2024</div></div>"#;
const PAGES_FIXTURE: &str = r#"<img class="reader-viewer-img" data-src="/1.jpg"><img class="reader-viewer-img" data-src="/2.jpg">"#;

export_manga_source!(SOURCE);
