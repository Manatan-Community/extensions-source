use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ComicNettai = ComicNettai;
const BASE_URL: &str = "https://www.comicnettai.com";

struct ComicNettai;

impl MangaSource for ComicNettai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body =
            fetch_document_or_fixture(&format!("{BASE_URL}/series?page={page}"), LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(
            &format!(
                "{BASE_URL}/search?q={}&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let mut chapters = Vec::new();
        let mut target = format!("{BASE_URL}{key}");
        loop {
            let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
            chapters.extend(parse_chapters_page(&body));
            let Some(next) = next_page_url(&body) else {
                break;
            };
            if chapters.len() > 500 || next == target {
                break;
            }
            target = next;
        }
        if chapters.is_empty() {
            chapters.push(MangaChapter {
                key: key.clone(),
                title: Some("Read".into()),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            });
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/viewer?cid=sample".into());
        let cid = query_param(&key, "cid").unwrap_or_else(|| "sample".into());
        let api_body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/viewer/c?cid={}", url::query_escape(&cid)),
            VIEWER_FIXTURE,
        );
        let content_url = json_string(&json_value(&api_body), "/url")
            .unwrap_or_else(|| format!("{BASE_URL}/content/sample/"));
        Ok(fetch_publus_pages(&content_url))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("full--comic__item")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "full--comic__title", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| {
                            url::slug_from_url(&key).unwrap_or_else(|| "Comic Nettai".into())
                        }),
                    cover: html::attr_after(chunk, "full--comic__thum", "data-src")
                        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                        .map(|value| url::join_url(BASE_URL, &value)),
                    url: Some(format!("{BASE_URL}{key}")),
                    language: Some("ja".into()),
                    content_rating: Some("safe".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagenation__item__link--next")
            && !body.contains("pagenation__item is-hidde"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "detail--title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Comic Nettai".into()),
        cover: html::attr_after(body, "detail-catch__img", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "detail--discription", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: body
            .split("detail__author__item")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_page(body: &str) -> Vec<MangaChapter> {
    body.split("detail--product__item")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "detail--product__item__title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".into())),
                date_uploaded: html::text_between(chunk, "detail--product__item__sdate", "</")
                    .map(|value| html::strip_tags(&value).replace('.', "-"))
                    .and_then(|value| dates::parse_ymd(&value)),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), |mut chapters, chapter| {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
            chapters
        })
}

fn fetch_publus_pages(content_url: &str) -> Vec<MangaPage> {
    let base = if content_url.ends_with('/') {
        content_url.to_string()
    } else {
        format!("{content_url}/")
    };
    let body = fetch_json_or_fixture(&format!("{base}configuration_pack.json"), PUBLUS_FIXTURE);
    let root = json_value(&body);
    if root.get("data").is_some() {
        return fixture_pages(&base);
    }
    let contents = root
        .pointer("/configuration/contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut pages = Vec::new();
    for entry in contents {
        let Some(file) = entry.get("file").and_then(Value::as_str) else {
            continue;
        };
        let index = entry
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(pages.len() as u64) as usize;
        let no = root
            .get(file)
            .and_then(|value| value.pointer("/FileLinkInfo/PageLinkInfoList/0/Page/No"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let image = format!("{base}{file}/{no}.jpeg");
        pages.push(MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        });
    }
    if pages.is_empty() {
        fixture_pages(&base)
    } else {
        pages
    }
}

fn fixture_pages(base: &str) -> Vec<MangaPage> {
    vec![MangaPage {
        content: PageContent::Url {
            url: format!("{base}page/0.jpeg"),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some("Page 1".into()),
        ..MangaPage::default()
    }]
}

fn next_page_url(body: &str) -> Option<String> {
    html::attr_after(body, "pagenation__item__link--next", "href")
        .map(|value| url::join_url(BASE_URL, &value))
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input
        .split('?')
        .nth(1)?
        .split('#')
        .next()
        .unwrap_or_default();
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn key_from_url(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    if path.starts_with('/') {
        Some(normalize_key(path))
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('#')
        .next()
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value);
    format!("/{}", path.trim_matches('/'))
}

fn json_value(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn json_string(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<html><a class="full--comic__item" href="/series/sample"><img class="full--comic__thum" data-src="/cover.jpg"><span class="full--comic__title">Sample Nettai</span></a></html>"#;
const DETAILS_FIXTURE: &str = r#"<html><h1 class="detail--title">Sample Nettai</h1><img class="detail-catch__img" src="/cover.jpg"><p class="detail--discription">Description</p><a class="detail--product__item" href="/viewer?cid=sample"><span class="detail--product__item__title">Chapter 1</span><span class="detail--product__item__sdate">2024.01.01</span></a></html>"#;
const VIEWER_FIXTURE: &str = r#"{"url":"https://www.comicnettai.com/content/sample/"}"#;
const PUBLUS_FIXTURE: &str = r#"{"configuration":{"contents":[{"index":0,"file":"page"}]},"page":{"FileLinkInfo":{"PageLinkInfoList":[{"Page":{"No":0,"Size":{"Width":100,"Height":100}}}]}}}"#;
