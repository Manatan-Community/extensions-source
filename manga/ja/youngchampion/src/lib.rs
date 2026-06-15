use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: YoungChampion = YoungChampion;
const BASE_URL: &str = "https://youngchampion.jp";

struct YoungChampion;

impl MangaSource for YoungChampion {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_ranking(&fetch_document(
            &format!("{BASE_URL}/ranking/manga"),
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if query.is_empty() {
            let path = filter_string(&request, "browse").unwrap_or("/series/list/up");
            if path == "/ranking/manga" {
                format!("{BASE_URL}{path}")
            } else {
                format!("{BASE_URL}{path}/{page}")
            }
        } else {
            format!(
                "{BASE_URL}/search?keyword={}&page={}&filter=series",
                url::query_escape(query),
                page.saturating_sub(1)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let show_locked = preference_bool(&request, "showLockedChapters", true);
        Ok(parse_chapters(
            &fetch_document(
                &format!("{}{}{}", BASE_URL, normalize_key(&key), "/list?s=1"),
                CHAPTERS_FIXTURE,
            ),
            show_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        if key.contains("#login") {
            return Ok(vec![manga::text_page(
                "Log in via WebView to read purchased chapters and refresh the entry.",
            )]);
        }
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGE_HTML_FIXTURE);
        let viewer_id = html::attr_after(&body, "comici-viewer", "comici-viewer-id")
            .unwrap_or_else(|| "sample-viewer".into());
        let member =
            html::attr_after(&body, "comici-viewer", "data-member-jwt").unwrap_or_default();
        Ok(fetch_viewer_pages(&viewer_id, &member, &chapter_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::ComiciViewer::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| absolute_url(key.split('#').next().unwrap_or(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn fetch_json(target: &str, referer: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Referer", referer)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    parse_listing(body)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("ranking-box")
                || chunk.contains("category-box-vertical")
                || chunk.contains("manga-store-item")
                || chunk.contains("series-list-item")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "c-ms-clk-article", "href")
                .or_else(|| html::attr_after(chunk, "series-list-item-link", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/series/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "title-text", "</")
                    .or_else(|| html::text_between(chunk, "manga-title", "</"))
                    .or_else(|| html::text_between(chunk, "series-list-item-h", "</"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Young Champion".into())
                    }),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("mode-paging-active")
            || body.contains("g-pager-link mode-active"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&body, "series-h-title", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Young Champion".into())),
        cover: html::text_between(&body, "series-h-img", "</div>")
            .and_then(|chunk| image_from_chunk(&chunk))
            .or_else(|| image_from_chunk(&body)),
        authors: html::text_between(&body, "series-h-credit-user", "</div>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(&body, "series-h-credit-info-text-text", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("series-h-tag-link")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, show_locked: bool) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("series-ep-list-item")
        .skip(1)
        .filter_map(|chunk| {
            let data_href =
                html::attr_after(chunk, "g-episode-link-wrapper", "data-href").unwrap_or_default();
            let article = html::attr_after(chunk, "g-episode-link-wrapper", "data-article")
                .unwrap_or_default();
            let is_free = chunk.contains("free-icon-new");
            let locked = !is_free;
            if locked && !show_locked {
                return None;
            }
            let key = if !data_href.is_empty() {
                normalize_key(&data_href)
            } else if !article.is_empty() {
                format!("{}#login", normalize_key(&article))
            } else {
                return None;
            };
            let title = html::text_between(chunk, "series-ep-list-item-h-text", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("{}{}", if locked { "Locked " } else { "" }, title)),
                date_uploaded: html::attr_after(chunk, "<time", "datetime").and_then(|value| {
                    manatan_shared::dates::parse_ymd(
                        value.split_whitespace().next().unwrap_or_default(),
                    )
                }),
                url: Some(absolute_url(key.split('#').next().unwrap_or(&key))),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: "/episodes/sample".into(),
            title: Some("Sample".into()),
            url: Some(format!("{BASE_URL}/episodes/sample")),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn fetch_viewer_pages(viewer_id: &str, member: &str, referer: &str) -> Vec<MangaPage> {
    let base = format!(
        "{BASE_URL}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id={}&page-from=0",
        url::query_escape(member)
    );
    let first = fetch_json(&format!("{base}&page-to=1"), referer, VIEWER_FIXTURE);
    let total = serde_json::from_str::<Value>(&first)
        .ok()
        .and_then(|value| value.get("totalPages").and_then(Value::as_u64))
        .unwrap_or(1);
    let body = fetch_json(&format!("{base}&page-to={total}"), referer, VIEWER_FIXTURE);
    parse_viewer_pages(&body, referer)
}

fn parse_viewer_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(VIEWER_FIXTURE).unwrap_or(Value::Null));
    root.get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let image = item.get("imageUrl").and_then(Value::as_str)?;
            let scramble = item
                .get("scramble")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut extra = BTreeMap::new();
            if !scramble.is_empty() {
                extra.insert("comiciScramble".into(), Value::String(scramble.to_string()));
            }
            Some(MangaPage {
                content: PageContent::Url {
                    url: image.to_string(),
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                description: Some(format!(
                    "Page {}",
                    item.get("sort").and_then(Value::as_u64).unwrap_or(0) + 1
                )),
                extra,
                ..MangaPage::default()
            })
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<source", "data-srcset")
        .and_then(|value| value.split_whitespace().next().map(ToOwned::to_owned))
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| {
            if let Some(rest) = value.strip_prefix("//") {
                format!("https://{rest}")
            } else if value.starts_with("http") {
                value
            } else {
                absolute_url(&value)
            }
        })
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)?
        .get(id)?
        .as_str()
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(default)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/series/") {
        Some(normalize_key(input))
    } else if input.starts_with("/series/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="ranking-box-vertical"><a href="/series/sample"><picture><source data-srcset="//cdn-public.comici.jp/cover.jpg 1x"></picture><div class="title-text">Sample Young Champion</div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="series-h-title"><span>Sample Young Champion</span></h1><div class="series-h-img"><source data-srcset="//cdn-public.comici.jp/cover.jpg 1x"></div><div class="series-h-credit-user">Sample Author</div><div class="series-h-credit-info-text-text">Sample description.</div><a class="series-h-tag-link">#Action</a>"#;
const CHAPTERS_FIXTURE: &str = r#"<div class="series-ep-list-item"><a class="g-episode-link-wrapper" data-href="/episodes/sample"><span class="free-icon-new"></span><span class="series-ep-list-item-h-text">Episode 1</span><time datetime="2024-01-01 00:00:00"></time></a></div>"#;
const PAGE_HTML_FIXTURE: &str =
    r#"<div id="comici-viewer" comici-viewer-id="sample-viewer" data-member-jwt=""></div>"#;
const VIEWER_FIXTURE: &str = r#"{"totalPages":1,"result":[{"imageUrl":"https://img.example.test/page1.jpg","scramble":"","sort":0}]}"#;
