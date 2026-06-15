use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
};
use manatan_shared::{dates, html, js, manga, url};
use serde_json::Value;

const SOURCE: Dm5 = Dm5;
const DEFAULT_BASE_URL: &str = "https://www.dm5.cn";
const CONTENT_RATING: &str = "adult";

struct Dm5;

impl MangaSource for Dm5 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{base}/manhua-list-s2-p{page}/")
        } else {
            format!("{base}/manhua-list-p{page}/")
        };
        let body = fetch_document(&target, &request, LIST_FIXTURE);
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
        if query.starts_with(&base) {
            let key = normalize_key(query, &base);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, &request, DETAILS_FIXTURE),
                    &key,
                    &base,
                )],
                has_next_page: false,
            });
        }
        let target = format!(
            "{base}/search?title={}&language=1&page={}",
            url::query_escape(query),
            page(&request)
        );
        let body = fetch_document(&target, &request, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manhua-sample/".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(&base, &key), &request, DETAILS_FIXTURE),
            &key,
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manhua-sample/".to_string());
        let body = fetch_document(&url::join_url(&base, &key), &request, DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body, &base);
        if preference_bool(&request, "sortChapter") {
            chapters.sort_by_key(|chapter| {
                chapter
                    .key
                    .trim_matches('/')
                    .trim_start_matches('m')
                    .trim_end_matches('/')
                    .parse::<u64>()
                    .unwrap_or_default()
            });
            chapters.reverse();
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/m1/".to_string());
        let target = url::join_url(&base, &key);
        let body = fetch_document(&target, &request, PAGES_FIXTURE);
        Ok(resolve_pages(&body, &target, &base, &request))
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
        if input.contains("dm5.") && input.contains("/manhua-") {
            let base = base_url(&request);
            let key = normalize_key(input, &base);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, &request, DETAILS_FIXTURE),
                    &key,
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("mirror"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("https://www.dm5."))
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

fn client(base: &str) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept-Language", "zh-TW")
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, request: &Value, fixture: &str) -> String {
    client(&base_url(request))
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_script(target: &str, request: &Value, referer: &str) -> Option<String> {
    client(&base_url(request))
        .get(target)
        .xhr()
        .referer(referer)
        .header("Accept", "*/*")
        .send_text()
        .ok()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn normalize_key(input: &str, base: &str) -> String {
    let path = input
        .strip_prefix(base)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn parse_cards(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("mh-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href, base);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| {
                        html::text_between(chunk, "title", "</a>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "动漫屋".to_string())
                    }),
                cover: html::attr_after(chunk, "mh-cover", "style")
                    .and_then(|style| {
                        style
                            .split("url(")
                            .nth(1)
                            .map(|s| s.trim_matches([')', '"', '\'']).to_string())
                    })
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(base, &image)),
                url: Some(url::join_url(base, &key)),
                language: Some("zh".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn has_next_page(body: &str) -> bool {
    body.contains("page-pagination")
        && (body.contains(">&gt;<") || body.contains(">><") || body.contains("下一页"))
}

fn parse_details(body: &str, key: &str, base: &str) -> CatalogItem {
    let detail = body.split("banner_detail_form").nth(1).unwrap_or(body);
    let author = html::text_between(detail, "subtitle", "</").map(|value| html::strip_tags(&value));
    CatalogItem {
        key: normalize_key(key, base),
        title: html::text_between(detail, "title", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "动漫屋".to_string()),
        cover: html::attr_after(detail, "<img", "src").map(|image| url::join_url(base, &image)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        tags: detail
            .split("p class=\"tip")
            .nth(1)
            .unwrap_or(detail)
            .split("<a")
            .skip(1)
            .filter_map(|part| html::text_between(part, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        description: html::text_between(detail, "content", "</p>")
            .map(|value| html::strip_tags(&value)),
        status: if detail.contains("连载中") {
            ItemStatus::Ongoing
        } else if detail.contains("已完结") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(base, key)),
        language: Some("zh".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    let container = body.split("chapterlistload").nth(1).unwrap_or(body);
    let scanlator_titles = body
        .split("detail-list-title")
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", "</"))
        .map(|value| {
            html::strip_tags(&value)
                .split('（')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect::<Vec<_>>();
    container
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href, base);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "title", "</p>")
                        .map(|value| html::strip_tags(&value))
                        .unwrap_or_else(|| html::strip_tags(chunk)),
                ),
                date_uploaded: html::text_between(chunk, "tip", "</p>")
                    .and_then(|value| dates::parse_ymd(&html::strip_tags(&value))),
                scanlators: scanlator_titles.first().cloned().into_iter().collect(),
                is_locked: chunk.contains("detail-lock") || chunk.contains("view-lock"),
                url: Some(url::join_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str, base: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("load-src") || chunk.contains("barChapter"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty())
        .map(|image| url::join_url(base, &image))
        .collect::<Vec<_>>();
    if images.is_empty() && body.contains("DM5_IMAGE_COUNT") {
        images = dm5_chapterfun_urls(body, referer);
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn resolve_pages(body: &str, referer: &str, base: &str, request: &Value) -> Vec<MangaPage> {
    let mut pages = parse_pages(body, referer, base);
    if !pages
        .iter()
        .any(|page| page_url(page).is_some_and(|url| url.contains("chapterfun.ashx")))
    {
        return pages;
    }

    let mut resolved = Vec::new();
    for page in pages.drain(..) {
        let Some(script_url) = page_url(&page).filter(|url| url.contains("chapterfun.ashx")) else {
            resolved.push(page);
            continue;
        };
        let Some(script) = fetch_script(script_url, request, referer) else {
            continue;
        };
        for image in dm5_script_image_urls(&script, base) {
            resolved.push(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                description: Some(format!("Page {}", resolved.len() + 1)),
                ..MangaPage::default()
            });
        }
    }
    resolved
}

fn page_url(page: &MangaPage) -> Option<&str> {
    match &page.content {
        PageContent::Url { url, .. } => Some(url),
        _ => None,
    }
}

fn dm5_script_image_urls(script: &str, base: &str) -> Vec<String> {
    let mut candidates = vec![script.to_string()];
    candidates.extend(js::extract_dean_edwards_payloads(script));
    candidates
        .iter()
        .flat_map(|payload| extract_image_urls(payload, base))
        .fold(Vec::new(), |mut out, url| {
            if !out.contains(&url) {
                out.push(url);
            }
            out
        })
}

fn extract_image_urls(input: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let mut rest = input;
        while let Some(start) = rest.find(quote) {
            rest = &rest[start + 1..];
            let Some(end) = rest.find(quote) else {
                break;
            };
            let value = &rest[..end];
            if looks_like_image_url(value) {
                out.push(url::join_url(base, value));
            }
            rest = &rest[end + 1..];
        }
    }
    out
}

fn looks_like_image_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.starts_with("http") || lower.starts_with("//") || lower.starts_with('/'))
        && (lower.contains(".jpg")
            || lower.contains(".jpeg")
            || lower.contains(".png")
            || lower.contains(".webp"))
}

fn dm5_chapterfun_urls(body: &str, reader_url: &str) -> Vec<String> {
    let script = body
        .split("<script")
        .find(|part| part.contains("DM5_MID"))
        .unwrap_or(body);
    if !script.contains("DM5_VIEWSIGN_DT") {
        return Vec::new();
    }
    let cid = js_var(script, "DM5_CID").unwrap_or_default();
    let mid = js_var(script, "DM5_MID").unwrap_or_default();
    let dt = js_var(script, "DM5_VIEWSIGN_DT").unwrap_or_default();
    let sign = js_var(script, "DM5_VIEWSIGN").unwrap_or_default();
    let count = js_var(script, "DM5_IMAGE_COUNT")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    (1..=count)
        .map(|page| format!("{}/chapterfun.ashx?cid={cid}&page={page}&key=&language=1&gtk=6&_cid={cid}&_mid={mid}&_dt={}&_sign={}", reader_url.trim_end_matches('/'), url::query_escape(&dt), url::query_escape(&sign)))
        .collect()
}

fn js_var(script: &str, name: &str) -> Option<String> {
    let rest = script.split(&format!("var {name}=")).nth(1)?;
    Some(
        rest.trim()
            .trim_start_matches(['"', '\''])
            .split([';', '"', '\''])
            .next()?
            .to_string(),
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<ul class="mh-list"><li><div class="mh-item"><p class="mh-cover" style="background-image: url(https://www.dm5.cn/cover.jpg)"></p><h2 class="title"><a href="/manhua-sample/" title="Sample DM5">Sample DM5</a></h2></div></li></ul><div class="page-pagination"><a>></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="banner_detail_form"><p class="title">Sample DM5</p><img src="/cover.jpg"><p class="subtitle"><a>Author</a></p><p class="tip"><a>Action</a><span><span>连载中</span></span></p><p class="content">Summary<span></span></p></div><div id="chapterlistload"><ul><li><a href="/m1/"><p class="title">Chapter 1</p><p class="tip">2024-01-01</p></a></li></ul></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="barChapter"><img class="load-src" data-src="https://www.dm5.cn/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
