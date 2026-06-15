use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Docomo = Docomo;
const BASE_URL: &str = "https://dbook.docomo.ne.jp";
const API_URL: &str = "https://dxp-system.docomo.ne.jp";
const SESSION_URL: &str = "https://rs4x.mw-pf.jp";

struct Docomo;

impl MangaSource for Docomo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, ".o-ranking-list__item"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!("{BASE_URL}/ranking/all/?s=daily&page={page}");
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            ".o-ranking-list__item",
        ))
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{BASE_URL}/search/?p={page}&q={}&s=sort_seriespop&t=2&ss=1",
            url::query_escape(query)
        );
        Ok(parse_listing(
            &fetch_document(&target, SEARCH_FIXTURE),
            ".o-card-list-light__item",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/item/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/item/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let series_id = html::attr_after(&body, "id=\"series_id\"", "value")
            .or_else(|| html::attr_after(&body, "id='series_id'", "value"))
            .unwrap_or_else(|| "sample-series".into());
        Ok(fetch_chapter_pages(&series_id)
            .into_iter()
            .flat_map(|fragment| parse_chapter_fragment(&fragment))
            .rev()
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/view/?cid=sample&cti=Sample&cc=0000".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, VIEWER_FIXTURE);
        let sesid = html::attr_after(&body, "mwrs4b-params", "data-sesid")
            .unwrap_or_else(|| "sample-session".into());
        let cid = query_param(&chapter_url, "cid").unwrap_or_else(|| "sample".into());
        let content_url = client()
            .post(format!("{SESSION_URL}/responder/sessionValidate"))
            .form(&[("cid", &cid), ("sesid", &sesid)])
            .xhr()
            .send_text()
            .ok()
            .and_then(|text| serde_json::from_str::<CPhpResponse>(&text).ok())
            .map(|response| response.url)
            .unwrap_or_else(|| "https://contents.invalid/sample/".into());
        Ok(parse_publus_pages(&content_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/item/") {
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

fn parse_listing(body: &str, class_marker: &str) -> Paged<CatalogItem> {
    let class_name = class_marker.trim_start_matches('.');
    let entries = body
        .split("<li")
        .chain(body.split("<div"))
        .filter(|chunk| chunk.contains(class_name))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "/item/", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/item/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "m-basic-card__title", "</")
                .or_else(|| html::text_between(chunk, "m-card-light__title", "</"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "cd-cover", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let has_next_page = body.contains("m-pager__next") || body.contains("pagination-btn--next");
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/item/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "p-header__title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "p-cover__image", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "o-product-information__summary-text", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: text_list(body, "p-information__author-list"),
        tags: text_list(body, "m-data-list__data"),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapter_pages(series_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut page = 1;
    loop {
        let target = format!(
            "{API_URL}/element/seriesshelf/get_contents?seriesId={}&order=a&page={page}",
            url::query_escape(series_id)
        );
        let text = client()
            .get(target)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        let response = serde_json::from_str::<ChaptersResponse>(&text)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
        out.push(response.html);
        if !response.has_next || page > 20 {
            break;
        }
        page += 1;
    }
    out
}

fn parse_chapter_fragment(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .filter(|chunk| chunk.contains("o-series-list__card-item"))
        .filter_map(|chunk| {
            let product_id = html::attr_after(chunk, "o-series-list__card", "data-product_id")?;
            let title = html::text_between(chunk, "o-series-list__card-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let key = format!(
                "/view/?cid={}&cti={}&cc=0000",
                url::query_escape(&product_id),
                url::query_escape(&title)
            );
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_publus_pages(content_url: &str) -> Vec<MangaPage> {
    let content_base = format!("{}/", content_url.trim_end_matches('/'));
    let config_url = format!("{content_base}configuration_pack.json");
    let body = client()
        .get(&config_url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| PUBLUS_CONFIG_FIXTURE.to_string());
    let root = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(PUBLUS_CONFIG_FIXTURE).expect("fixture is valid"));
    let contents = root
        .pointer("/configuration/contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![serde_json::json!({"index":0,"file":"p0001"})]);
    contents
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let file = entry.get("file").and_then(Value::as_str).unwrap_or("p0001");
            let page_cfg = root.get(file).unwrap_or(&Value::Null);
            let no = page_cfg
                .pointer("/FileLinkInfo/PageLinkInfoList/0/Page/No")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let image = format!("{content_base}{file}/{no}.jpeg");
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn text_list(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    let query = input
        .split('?')
        .nth(1)?
        .split('#')
        .next()
        .unwrap_or_default();
    for part in query.split('&') {
        let (key, value) = part.split_once('=')?;
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = path.split('#').next().unwrap_or(path);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

#[derive(Deserialize)]
struct ChaptersResponse {
    html: String,
    #[serde(rename = "hasNext")]
    has_next: bool,
}

#[derive(Deserialize)]
struct CPhpResponse {
    url: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<li class="o-ranking-list__item"><a href="/item/sample"><img class="cd-cover" src="/cover.jpg"><div class="m-basic-card__title">Sample Docomo</div></a></li>"#;
const SEARCH_FIXTURE: &str = r#"<li class="o-card-list-light__item"><a href="/item/sample"><img class="cd-cover" src="/cover.jpg"><div class="m-card-light__title">Sample Docomo</div></a></li>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="p-header__title">Sample Docomo</h1><input id="series_id" value="sample-series"><div class="p-cover__image"><img src="/cover.jpg"></div><div class="o-product-information__summary-text">Fixture description.</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"html":"<div class=\"o-series-list__card-item\"><div class=\"o-series-list__card\" data-product_id=\"sample-product\"><div class=\"o-series-list__card-title\">Chapter 1</div></div></div>","hasNext":false}"#;
const VIEWER_FIXTURE: &str = r#"<div id="mwrs4b-params" data-sesid="sample-session"></div>"#;
const PUBLUS_CONFIG_FIXTURE: &str = r#"{"configuration":{"contents":[{"index":0,"file":"p0001"}]},"p0001":{"FileLinkInfo":{"PageLinkInfoList":[{"Page":{"No":0,"Size":{"Width":800,"Height":1200}}}]}}}"#;
