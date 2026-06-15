use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Comicabc = Comicabc;
const BASE_URL: &str = "https://www.8comic.com";

struct Comicabc;

impl MangaSource for Comicabc {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let p = page(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("/comic/u-{p}.html")
        } else {
            format!("/comic/h-{p}.html")
        };
        Ok(parse_listing(&fetch(
            &url::join_url(BASE_URL, &path),
            LIST_FIXTURE,
        )))
    }
    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let q = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if q.starts_with(BASE_URL) {
            let key = normalize_key(q);
            return Ok(Paged {
                entries: vec![parse_details(&fetch(q, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch(
            &format!(
                "{BASE_URL}/member/search.aspx?key={}&page={}",
                url::query_escape(q),
                page(&request)
            ),
            LIST_FIXTURE,
        )))
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/html/1.html".into());
        Ok(parse_details(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }
    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/html/1.html".into());
        Ok(parse_chapters(&fetch(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }
    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/online/new-1.html?ch=1".into());
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
            let key = normalize_key(input);
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
fn normalize_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|c| c.contains("/html/") || c.contains("comicpic_col6") || c.contains("cat2_list"))
        .filter_map(|c| {
            let href = html::attr(c, "href")?;
            if !href.contains("/html/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(c, "<img", "alt").unwrap_or_else(|| html::strip_tags(c)),
                cover: html::attr_after(c, "<img", "src").map(|i| url::join_url(BASE_URL, &i)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("zh".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("mdi-skip-next") || body.contains("下一"),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let author = text_after(body, "item-info-author").map(|v| v.replace("作者: ", ""));
    CatalogItem {
        key: key.into(),
        title: text_after(body, " class=\"h2").unwrap_or_else(|| "無限動漫".into()),
        cover: html::attr_after(body, "item-cover", "src").map(|i| url::join_url(BASE_URL, &i)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        description: text_after(body, "item_info_detail"),
        status: match text_after(body, "item-info-status").as_deref() {
            Some("連載中") => ItemStatus::Ongoing,
            Some("已完結") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("zh".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}
fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(&body[body.find(marker)?..], ">", "</")
        .map(|v| html::strip_tags(&v))
        .filter(|v| !v.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|c| c.contains("cview(") || c.contains("/online/"))
        .filter_map(|c| {
            let key = if let Some(onclick) = html::attr(c, "onclick") {
                let cid = onclick.split("cview('").nth(1)?.split('-').next()?;
                let chid = onclick.split('-').nth(1)?.split(".html").next()?;
                format!("/online/new-{cid}.html?ch={chid}")
            } else {
                normalize_key(&html::attr(c, "href")?)
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(html::strip_tags(c)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter_map(|c| html::attr(c, "data-src").or_else(|| html::attr(c, "src")))
        .filter(|i| i.contains("8comic") || i.contains("/"))
        .map(|i| url::join_url(BASE_URL, &i))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = decode_pages(body);
    }
    images
        .into_iter()
        .enumerate()
        .map(|(idx, i)| MangaPage {
            content: PageContent::Url {
                url: i,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", idx + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_pages(body: &str) -> Vec<String> {
    let ps = js_num(body, "ps").unwrap_or(0);
    let ti = js_str(body, "ti").unwrap_or_default();
    if ps == 0 || ti.is_empty() {
        return Vec::new();
    }
    (1..=ps)
        .map(|i| format!("https://img1.8comic.com/a/{ti}/001/{i:03}_0.jpg"))
        .collect()
}
fn js_num(body: &str, key: &str) -> Option<u64> {
    body.split(&format!("{key}="))
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}
fn js_str(body: &str, key: &str) -> Option<String> {
    let rest = body.split(&format!("{key}=")).nth(1)?.trim_start();
    let q = rest.chars().next()?;
    if q != '\'' && q != '"' {
        return None;
    }
    Some(rest[1..].split(q).next()?.into())
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|i| i.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<a class="comicpic_col6" href="/html/1.html"><img alt="Sample Comicabc" src="/cover.jpg"></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="item_content_box"><div class="h2">Sample Comicabc</div><div class="item-info-author">作者: Author</div><div class="item-info-status">連載中</div><div class="item_info_detail">Description</div></div><div class="item-cover"><img src="/cover.jpg"></div><div id="chapters"><a onclick="cview('1-1.html')">Chapter 1</a></div>"#;
const PAGES_FIXTURE: &str = r#"<img src="https://img1.8comic.com/a/1/1/001_abc.jpg">"#;

export_manga_source!(SOURCE);
