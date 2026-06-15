use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Zerobyw = Zerobyw;
const DEFAULT_BASE_URL: &str = "http://www.zerobywgbo2.com";

struct Zerobyw;

impl MangaSource for Zerobyw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let body = fetch(
            &base,
            &format!("{base}/pc/pc/?page={}", page(&request)),
            LIST_FIXTURE,
        );
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
        let mut params = Vec::new();
        if !query.is_empty() {
            params.push(format!("keyword={}", url::query_escape(query)));
        } else {
            let filters = request.get("filters").unwrap_or(&Value::Null);
            for key in ["category_id", "jindu", "shuxing"] {
                if let Some(value) = filters
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    params.push(format!("{key}={}", url::query_escape(value)));
                }
            }
        }
        params.push(format!("page={}", page(&request)));
        let body = fetch(
            &base,
            &format!("{base}/pc/pc/?{}", params.join("&")),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body, &base),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/details/?kuid=1".into());
        Ok(parse_details(
            &fetch(&base, &url::join_url(&base, &key), DETAILS_FIXTURE),
            &key,
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/details/?kuid=1".into());
        let body = fetch(&base, &url::join_url(&base, &key), DETAILS_FIXTURE);
        let mut chapters = body
            .split("<a")
            .filter(|chunk| chunk.contains("/view/index.php"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let chapter_key = key_from_url(&href)?;
                let title = clean_title(&html::strip_tags(chunk));
                Some(MangaChapter {
                    key: chapter_key.clone(),
                    title: Some(if title.is_empty() {
                        "Chapter".into()
                    } else {
                        title
                    }),
                    url: Some(url::join_url(&base, &chapter_key)),
                    ..MangaChapter::default()
                })
            })
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/view/index.php?kuid=1&cid=1".into());
        let body = fetch(&base, &url::join_url(&base, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &base))
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
        if let Some(key) = key_from_url(input).filter(|key| key.contains("/details/")) {
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
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}
fn fetch(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
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
    if !(input.contains("/details/") || input.contains("/view/index.php")) {
        return None;
    }
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or(input);
    Some(format!("/{}", path.trim_start_matches('/')))
}

fn parse_cards(body: &str, base: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .filter(|chunk| chunk.contains("/details/?kuid="))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            let raw_title = html::text_between(chunk, "<h3", "</h3>")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_else(|| html::strip_tags(chunk));
            Some(CatalogItem {
                key: key.clone(),
                title: clean_title(&raw_title),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(base, &image)),
                url: Some(url::join_url(base, &key)),
                language: Some("zh".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: &str, base: &str) -> CatalogItem {
    let lab_text = html::text_between(body, "flex-wrap text-sm", "</div>")
        .map(|v| html::strip_tags(&v))
        .unwrap_or_default();
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|v| clean_title(&html::strip_tags(&v)))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "zero搬运网".into()),
        cover: html::attr_after(body, "object-contain", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(base, &image)),
        authors: field_after(&lab_text, "作者:").into_iter().collect(),
        tags: lab_text
            .split_whitespace()
            .map(|v| v.trim_matches(',').to_string())
            .filter(|v| !v.is_empty())
            .collect(),
        description: html::text_between(body, "x-ref=\"summaryText\"", "</p>")
            .or_else(|| html::text_between(body, "x-ref='summaryText'", "</p>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: if lab_text.contains("已完结") {
            ItemStatus::Completed
        } else if lab_text.contains("连载中") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(base, key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .filter(|chunk| chunk.contains("manga-image"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(base, &image),
                context: None,
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn clean_title(title: &str) -> String {
    title.split('【').next().unwrap_or(title).trim().to_string()
}
fn field_after(text: &str, label: &str) -> Option<String> {
    text.split(label)
        .nth(1)
        .map(|v| v.split_whitespace().next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty())
}
fn has_next_page(body: &str) -> bool {
    body.contains("下一页")
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str =
    r#"<a href="/details/?kuid=1"><img src="/cover.jpg"><h3>Sample【12】</h3></a><a>下一页</a>"#;
const DETAILS_FIXTURE: &str = r#"<main><h1>Sample【12】</h1><img class="object-contain" src="/cover.jpg"><div class="flex-wrap text-sm"><span>作者: Author</span><span>连载中</span></div><p x-ref="summaryText">Sample description.</p><div class="grid"><a href="/view/index.php?kuid=1&cid=1">Chapter 1</a></div></main>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="image-container"><img class="manga-image" src="/page.jpg"></div>"#;
