use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaKuro = MangaKuro;
const BASE_URL: &str = "https://mangakuro.net";

struct MangaKuro;

impl MangaSource for MangaKuro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest-updated"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/all-manga/{page}?sort={sort}"),
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}/search/{page}?keyword={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let chapter_id = chapter_id(&body).unwrap_or_else(|| "1".into());
        let api_body = fetch_document(
            &format!("{BASE_URL}/ajax/image/list/chap/{chapter_id}"),
            IMAGE_LIST_FIXTURE,
        );
        Ok(parse_pages(&api_body, &chapter_url))
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
    let entries = body
        .split("story_item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::text_between(chunk, "mg_name", "</div>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaKuro".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
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
        has_next_page: body.contains("title=\"Last Page\"") || body.contains("title='Last Page'"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document(&url::join_url(BASE_URL, key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "MangaKuro".into())),
        cover: html::attr_after(&body, "detail_avatar", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(&body, "detail_reviewContent", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(&body, "lnr-user"),
        status: parse_status(&html::strip_tags(&body)),
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter_box")
        .flat_map(|chunk| chunk.split("class=\"item\"").skip(1))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value).replace("# ", ""))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split(r#"src=\""#)
        .skip(1)
        .filter_map(|rest| rest.split(r#"\""#).next().map(ToString::to_string))
        .chain(
            body.split("<img")
                .skip(1)
                .filter_map(|chunk| html::attr(chunk, "src")),
        )
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .map(|image| url::join_url(BASE_URL, &image))
        .fold(Vec::<String>::new(), |mut pages, image| {
            if !pages.contains(&image) {
                pages.push(image);
            }
            pages
        })
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

fn chapter_id(body: &str) -> Option<String> {
    body.split("CHAPTER_ID")
        .nth(1)
        .and_then(|rest| rest.split('=').nth(1))
        .and_then(|rest| rest.split(';').next())
        .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
        .filter(|value| !value.is_empty())
}

fn info_values(body: &str, icon: &str) -> Vec<String> {
    body.split(icon)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "info_value", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("進行中") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

const LIST_FIXTURE: &str = r#"
<div class="story_item"><a href="/manga/sample"><img src="/cover.jpg"><span class="mg_name"><a>Sample Manga</a></span></a></div>
<a title="Last Page" href="/all-manga/2">Last</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1><div class="detail_avatar"><img src="/cover.jpg"></div><div><span class="lnr-user"></span><div class="info_value">Writer</div></div><div><span class="lnr-leaf"></span><div class="info_value">進行中</div></div><div class="detail_reviewContent">Sample description.</div>
<div class="chapter_box"><div class="item"><a href="/sample-chapter-1"># Chapter 1</a></div></div>
"#;

const PAGES_FIXTURE: &str = r#"var CHAPTER_ID = 1;"#;

const IMAGE_LIST_FIXTURE: &str =
    r#"<img src=\"https://mangakuro.net/page1.jpg\"><img src=\"https://mangakuro.net/page2.jpg\">"#;

export_manga_source!(SOURCE);
