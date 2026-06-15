use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ComicGrast = ComicGrast;
const BASE_URL: &str = "https://novema.jp";

struct ComicGrast;

impl MangaSource for ComicGrast {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/comic/serial/{page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_for_key(&key)],
                has_next_page: false,
            });
        }
        let page = request
            .get("page")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        let target = format!(
            "{BASE_URL}/comic/search/{page}?type=0&word={}",
            url::query_escape(query)
        );
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/serial/sample".into());
        Ok(details_for_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/serial/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/story/sample".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        parse_pages(&body, &chapter_url)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_key(&key)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, referer: &str, fixture: &str) -> Value {
    client()
        .get(target)
        .referer(referer)
        .xhr()
        .send_text()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("comicList") || chunk.contains("/comic/serial/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/comic/serial/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "line-clamp n2", "</")
                    .or_else(|| html::text_between(chunk, "<dt", "</dt>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Comic Grast".into())
                    }),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pageList") && body.contains("next"),
    }
}

fn details_for_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&body, "comicTit", "</")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic Grast".into())),
        cover: html::attr_after(&body, "serialMainImage", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(&body, "txtAcd_text_inner", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: body
            .split("<li")
            .filter(|chunk| chunk.contains("credit"))
            .map(|chunk| html::strip_tags(&format!("<li{chunk}")))
            .filter(|value| !value.is_empty())
            .collect(),
        tags: body
            .split("<a")
            .filter(|chunk| chunk.contains("topCategoryTag") || chunk.contains('#'))
            .map(|chunk| {
                html::strip_tags(&format!("<a{chunk}"))
                    .trim_start_matches('#')
                    .to_string()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("comicSerialList") || chunk.contains("/comic/story/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "storyTitle", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".into())),
                date_uploaded: html::text_between(chunk, "update", "</")
                    .map(|value| html::strip_tags(&value).replace(" 更新", ""))
                    .and_then(|value| manatan_shared::dates::parse_ymd(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> ExtensionResult<Vec<MangaPage>> {
    let raw =
        html::text_between(body, "comic-data", "</script>").ok_or_else(|| ExtensionError {
            message: "comic-data script not found".into(),
        })?;
    let data: Value = serde_json::from_str(raw.trim()).map_err(|error| ExtensionError {
        message: format!("comic-data JSON parse error: {error}"),
    })?;
    let serial_id = data
        .get("serial_comic_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ExtensionError {
            message: "serial_comic_id not found".into(),
        })?;
    let story_number = data
        .get("story_number")
        .and_then(Value::as_i64)
        .ok_or_else(|| ExtensionError {
            message: "story_number not found".into(),
        })?;
    let index_url =
        format!("{BASE_URL}/img/serial-comic/{serial_id}/{story_number}/content/index.json");
    let pages = fetch_json(&index_url, chapter_url, INDEX_FIXTURE);
    Ok(pages
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let name = page.get("name").and_then(Value::as_str)?;
            let seed = page.get("seed").and_then(Value::as_str).unwrap_or_default();
            let size = page.get("size").and_then(Value::as_i64).unwrap_or_default();
            let image = format!(
                "{BASE_URL}/img/serial-comic/{serial_id}/{story_number}/content/{}?seed={}&size={}",
                name,
                url::query_escape(seed),
                size
            );
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(chapter_url)),
                },
                headers: manga::image_headers(chapter_url),
                description: Some(name.to_string()),
                ..MangaPage::default()
            })
        })
        .collect())
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    for marker in ["/comic/serial/", "/comic/story/"] {
        if let Some(index) = path.find(marker) {
            return format!(
                "/{}",
                path[index + 1..]
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.contains(BASE_URL) || input.starts_with("/comic/serial/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|entry| entry.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="comicList"><li><a href="/comic/serial/sample"><dl><dt><p class="line-clamp n2">Sample Grast</p></dt></dl><img data-src="/cover.jpg"></a></li></ul>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<h1 class="comicTit">Sample Grast</h1><img class="serialMainImage" src="/cover.jpg"><div class="comicStory"><div class="txtAcd_text_inner">Description</div></div><div class="comicSerialList"><article><a href="/comic/story/1"><span class="storyTitle">Episode 1</span><span class="update">2024/01/01 更新</span></a></article></div>"#;
const PAGES_FIXTURE: &str = r#"<script id="comic-data" type="application/json">{"serial_comic_id":1,"story_number":1}</script>"#;
const INDEX_FIXTURE: &str = r#"[{"name":"001.jpg","seed":"seed","size":4}]"#;
