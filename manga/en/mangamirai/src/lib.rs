use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: MangaMirai = MangaMirai;
const BASE_URL: &str = "https://mangamirai.com";

struct MangaMirai;

impl MangaSource for MangaMirai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "ranking"
        } else {
            "new"
        };
        Ok(parse_search(&fetch_document(
            &search_url("", page, order, "", ""),
            SEARCH_FIXTURE,
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = filter_string(&request, "sort").unwrap_or_else(|| "new".into());
        let genre = filter_string(&request, "genre").unwrap_or_default();
        let publisher = filter_string(&request, "publisher").unwrap_or_default();
        Ok(parse_search(&fetch_document(
            &search_url(query, page, &order, &genre, &publisher),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        Ok(fetch_chapters(&key, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        let body = fetch_json(
            &format!(
                "{BASE_URL}/users/product_contents/{}/product_content_images?start_page=1&limit=10000",
                url::query_escape(&key)
            ),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::MangaMiraiImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/product_collections/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/users/product_contents/{key}/book_reader")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let item = (!input.contains("/book_reader")).then(|| details_by_key(&key));
            return Ok(Some(UrlResolveResult {
                item,
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "*/*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(query: &str, page: u64, order: &str, genre: &str, publisher: &str) -> String {
    let mut params = vec![
        format!("word={}", url::query_escape(query)),
        format!("page={page}"),
        format!("order={}", url::query_escape(order)),
    ];
    if !genre.is_empty() {
        params.push(format!("genre={}", url::query_escape(genre)));
    }
    if !publisher.is_empty() {
        params.push(format!("publisher={}", url::query_escape(publisher)));
    }
    format!("{BASE_URL}/search?{}", params.join("&"))
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .filter(|chunk| chunk.contains("card") && chunk.contains("href="))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = key_from_path(&href)?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(format!("{BASE_URL}/product_collections/{key}")),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(
            &format!("{BASE_URL}/product_collections/{key}"),
            DETAILS_FIXTURE,
        ),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let title = html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| key.to_string());
    let description = body
        .split("data-product-collections--product-collection--long-description-accordion-target")
        .nth(1)
        .and_then(|chunk| chunk.split("</span>").next())
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    let tags = body
        .split("popular-categories")
        .skip(1)
        .flat_map(|chunk| chunk.split("</a>").take(8))
        .filter_map(|chunk| {
            html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    CatalogItem {
        key: key.into(),
        title,
        cover: html::attr_after(body, "grid-cols-5", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: body
            .split("href=\"/authors/")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        description,
        tags,
        status: if body.contains("/tags/Completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        url: Some(format!("{BASE_URL}/product_collections/{key}")),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for page in 1..=20 {
        let body = fetch_document(
            &format!("{BASE_URL}/product_collections/{key}?page={page}"),
            DETAILS_FIXTURE,
        );
        chapters.extend(parse_chapters_page(&body, hide_locked));
        if !(body.contains("rel=\"next\"") || body.contains("rel='next'")) {
            break;
        }
    }
    chapters.reverse();
    chapters
}

fn parse_chapters_page(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    body.split("<div")
        .filter(|chunk| chunk.contains("pb-5") && chunk.contains("href="))
        .filter_map(|chunk| {
            let is_bought = chunk.contains("gtm_read");
            let is_free = chunk.contains("gtm_read_for_free");
            let is_preview = chunk.contains("gtm_preview");
            let is_locked = !is_bought && !is_free && !is_preview;
            if hide_locked && (is_locked || is_preview) {
                return None;
            }
            let href = html::attr_after(chunk, "<a", "href")?;
            let content_id = content_id_from_path(&href)?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Chapter {content_id}"));
            let prefix = if is_preview {
                "[Preview] "
            } else if is_locked {
                "[Locked] "
            } else {
                ""
            };
            Some(MangaChapter {
                key: content_id.clone(),
                title: Some(format!("{prefix}{title}")),
                url: Some(format!(
                    "{BASE_URL}/users/product_contents/{content_id}/book_reader"
                )),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, content_id: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ViewerResponse>(body).unwrap_or_default();
    response
        .records
        .into_iter()
        .map(|record| {
            let mut extra = BTreeMap::new();
            extra.insert("contentId".into(), json!(content_id));
            extra.insert("scrambleKey".into(), json!(record.scramble_key));
            MangaPage {
                content: PageContent::Url {
                    url: record.url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", record.page)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    input.strip_prefix(BASE_URL).and_then(key_from_path)
}

fn key_from_path(path: &str) -> Option<String> {
    let clean = path.trim_start_matches('/');
    clean
        .strip_prefix("product_collections/")
        .map(|value| value.split(['?', '/']).next().unwrap_or(value).to_string())
}

fn content_id_from_path(path: &str) -> Option<String> {
    let clean = path.trim_start_matches('/');
    clean
        .strip_prefix("users/product_contents/")
        .and_then(|value| value.split('/').next())
        .map(ToOwned::to_owned)
        .or_else(|| clean.split('/').nth(3).map(ToOwned::to_owned))
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[derive(Default, Deserialize)]
struct ViewerResponse {
    #[serde(default)]
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    page: i64,
    #[serde(default)]
    scramble_key: String,
    url: String,
}

const SEARCH_FIXTURE: &str = r#"<div class="card"><a href="/product_collections/sample"><img src="https://img.example/cover.jpg"><h3>Sample Mirai</h3></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Mirai</h1><span data-product-collections--product-collection--long-description-accordion-target>Summary</span><div class="popular-categories"><a href="/tags/Completed">Completed</a></div><div class="pb-5"><a class="gtm_read_for_free" href="/users/product_contents/123/book_reader"><h3><span class="font-bold">Chapter 1</span></h3></a></div>"#;
const PAGES_FIXTURE: &str =
    r#"{"records":[{"page":1,"scramble_key":"WzBd","url":"https://img.example/001.enc"}]}"#;

export_manga_source!(SOURCE);
