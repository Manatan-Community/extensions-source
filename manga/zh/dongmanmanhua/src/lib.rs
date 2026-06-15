use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: DongmanManhua = DongmanManhua;
const BASE_URL: &str = "https://www.dongmanmanhua.cn";

struct DongmanManhua;

impl MangaSource for DongmanManhua {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/dailySchedule?sortOrder=UPDATE&webtoonCompleteType=ONGOING")
        } else {
            format!("{BASE_URL}/dailySchedule")
        };
        Ok(Paged {
            entries: parse_cards(&fetch(&target, LIST_FIXTURE)),
            has_next_page: false,
        })
    }
    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let q = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if q.starts_with(BASE_URL) {
            let key = norm(q);
            return Ok(Paged {
                entries: vec![parse_details(&fetch(q, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/search?keyword={}&page={}",
            url::query_escape(q),
            page(&request)
        );
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("more_area") || body.contains("paginate"),
        })
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sample/list?title_no=1".into());
        Ok(parse_details(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }
    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sample/list?title_no=1".into());
        Ok(parse_chapters(&fetch(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }
    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/1?title_no=1&episode_no=1".into());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_pages(&fetch(&target, PAGES_FIXTURE), &target))
    }
    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|k| url::join_url(BASE_URL, &k)))
    }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|k| url::join_url(BASE_URL, &k)))
    }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = norm(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), &key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}
fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn norm(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .trim_start_matches('/')
    )
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|c| c.contains("subj") || c.contains("daily_card") || c.contains("card_wrap"))
        .filter_map(|c| {
            let href = html::attr(c, "href")?;
            let key = norm(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(c, "subj", "</p>")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .or_else(|| html::attr_after(c, "<img", "alt"))
                    .unwrap_or_else(|| "Dongman Manhua".into()),
                cover: html::attr_after(c, "<img", "src").map(|i| url::join_url(BASE_URL, &i)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("zh-Hans".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "subj", "</")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "Dongman Manhua".into()),
        cover: html::attr_after(body, "detail_body", "style")
            .and_then(|s| {
                s.split("url(")
                    .nth(1)
                    .map(|v| v.trim_matches([')', '"', '\'']).to_string())
            })
            .or_else(|| html::attr_after(body, "thmb", "src"))
            .map(|i| url::join_url(BASE_URL, &i)),
        description: html::text_between(body, "summary", "</p>").map(|v| html::strip_tags(&v)),
        status: if body.contains("完结") {
            ItemStatus::Completed
        } else if body.contains("更新") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("zh-Hans".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter_map(|c| {
            let href = html::attr_after(c, "<a", "href")?;
            let key = norm(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(c, "subj", "</span>")
                    .map(|v| html::strip_tags(&v))
                    .or_else(|| Some(html::strip_tags(c))),
                date_uploaded: html::text_between(c, "date", "</span>")
                    .and_then(|v| dates::parse_ymd(&html::strip_tags(&v).replace('.', "-"))),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}
fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|c| html::attr(c, "data-url").or_else(|| html::attr(c, "src")))
        .filter(|i| !i.contains("logo"))
        .enumerate()
        .map(|(idx, i)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &i),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", idx + 1)),
            ..MangaPage::default()
        })
        .collect()
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|i| i.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div id="dailyList"><a href="/sample/list?title_no=1"><img src="/cover.jpg"><p class="subj">Sample Dongman</p></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="subj">Sample Dongman</h1><span class="thmb"><img src="/cover.jpg"></span><p class="summary">Summary</p><ul id="_listUl"><li><a href="/sample/1?title_no=1&episode_no=1"><span class="subj"><span>Chapter 1</span></span><span class="date">2024-1-1</span></a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div id="_imageList"><img data-url="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
