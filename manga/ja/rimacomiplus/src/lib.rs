use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionError,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: RimacomiPlus = RimacomiPlus;
const BASE_URL: &str = "https://rimacomiplus.jp";
const API_URL: &str = "https://rimacomiplus.jp/api";
const SEARCH_PAGE_SIZE: u64 = 20;

struct RimacomiPlus;

impl MangaSource for RimacomiPlus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        parse_listing_result(&fetch_document(&format!("{BASE_URL}/ranking/manga"))?)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let page = page(&request);
        if !query.is_empty() {
            let target = format!(
                "{API_URL}/search?q={}&page={page}&size={SEARCH_PAGE_SIZE}",
                url::query_escape(query)
            );
            return parse_search_json_result(&fetch_json(&target)?, page);
        }
        let path = filter_string(&request, "browse").unwrap_or("/ranking/manga");
        let target = if path.contains('?') {
            format!("{BASE_URL}{path}&page={page}")
        } else {
            format!("{BASE_URL}{path}")
        };
        parse_listing_result(&fetch_document(&target)?)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let hash = series_hash(&key);
        let show_locked = preference_bool(&request, "showLockedChapters", true);
        let show_login = preference_bool(&request, "showLoginRequiredChapters", true);
        fetch_chapters(&hash, show_locked, show_login)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        if key.contains("#login") {
            return Ok(vec![manga::text_page(
                "Log in via WebView to read this chapter.",
            )]);
        }
        let episode_id = key
            .trim_matches('/')
            .split('/')
            .next_back()
            .unwrap_or("sample");
        let episode = fetch_json(&format!("{API_URL}/episodes/{episode_id}"))?;
        let viewer_id =
            find_viewer_id(&episode).ok_or_else(|| err("could not find Comici viewer id"))?;
        fetch_viewer_pages(&viewer_id)
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
                item: Some(details_by_key(&key)?),
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

fn fetch_document(target: &str) -> ExtensionResult<String> {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .map_err(|error| err(&format!("fetch failed for {target}: {}", error.message)))
}

fn fetch_json(target: &str) -> ExtensionResult<String> {
    client()
        .get(target)
        .xhr()
        .send_text()
        .map_err(|error| err(&format!("fetch failed for {target}: {}", error.message)))
}

fn parse_listing_result(body: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let page = parse_listing(body);
    if page.entries.is_empty() {
        Err(err("no manga entries found in listing"))
    } else {
        Ok(page)
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<")
        .filter(|chunk| {
            chunk.contains("/series/")
                && (chunk.contains("href=") || chunk.contains("\\\"href\\\""))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")
                .or_else(|| json_string_after(chunk, "\"href\":\""))
                .filter(|href| href.contains("/series/"))?;
            Some(item_from_parts(&href, chunk))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pgLnkNext")
            || body.contains("pager-next")
            || body.contains("mode-icon"),
    }
}

fn parse_search_json_result(body: &str, page: u64) -> ExtensionResult<Paged<CatalogItem>> {
    let parsed = parse_search_json(body, page)?;
    if parsed.entries.is_empty() {
        Err(err("no manga entries found in search response"))
    } else {
        Ok(parsed)
    }
}

fn parse_search_json(body: &str, page: u64) -> ExtensionResult<Paged<CatalogItem>> {
    let root = parse_json(body)?;
    let result = root.pointer("/searchResult/series");
    let total = result
        .and_then(|v| v.get("total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entries = result
        .and_then(|v| v.get("series"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            Some(CatalogItem {
                key: format!("/series/{id}"),
                title: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("RimacomiPlus")
                    .to_string(),
                cover: first_image(item),
                url: Some(format!("{BASE_URL}/series/{id}")),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Ok(Paged {
        entries,
        has_next_page: total > page * SEARCH_PAGE_SIZE,
    })
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    let hash = series_hash(key);
    let root = parse_json(&fetch_json(&format!(
        "{API_URL}/episodes?seriesHash={hash}"
    ))?)?;
    let summary = root
        .pointer("/series/summary")
        .ok_or_else(|| err("series summary missing"))?;
    Ok(CatalogItem {
        key: format!("/series/{hash}"),
        title: summary
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("RimacomiPlus")
            .to_string(),
        cover: first_image(summary),
        authors: summary
            .get("author")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        description: summary
            .get("description")
            .and_then(Value::as_str)
            .map(parse_description)
            .filter(|value| !value.is_empty()),
        tags: summary
            .get("tag")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        status: if summary
            .get("isCompleted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/series/{hash}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn fetch_chapters(
    hash: &str,
    show_locked: bool,
    show_login: bool,
) -> ExtensionResult<Vec<MangaChapter>> {
    let details = parse_json(&fetch_json(&format!(
        "{API_URL}/episodes?seriesHash={hash}&episodeFrom=1&episodeTo=9999"
    ))?)?;
    let access = fetch_json(&format!(
        "{API_URL}/series/access?seriesHash={hash}&episodeFrom=1&episodeTo=9999"
    ))
    .ok()
    .and_then(|body| parse_json(&body).ok());
    let access_items = access
        .as_ref()
        .and_then(|value| value.pointer("/seriesAccess/episodeAccesses"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut chapters = details
        .pointer("/series/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let id = episode.get("id").and_then(Value::as_str)?;
            let access = access_items
                .iter()
                .find(|item| item.get("episodeId").and_then(Value::as_str) == Some(id));
            let has_access = access
                .and_then(|item| item.get("hasAccess"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let campaign = access
                .and_then(|item| item.get("isCampaign"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let locked = !has_access;
            if locked && campaign && !show_login {
                return None;
            }
            if locked && !campaign && !show_locked {
                return None;
            }
            let suffix = if locked && campaign { "#login" } else { "" };
            Some(MangaChapter {
                key: format!("/episodes/{id}{suffix}"),
                title: Some(format!(
                    "{}{}",
                    if locked && campaign {
                        "Login "
                    } else if locked {
                        "Locked "
                    } else {
                        ""
                    },
                    episode
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Chapter")
                )),
                date_uploaded: episode
                    .get("datePublished")
                    .and_then(Value::as_i64)
                    .map(|value| value * 1000),
                url: Some(format!("{BASE_URL}/episodes/{id}")),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    if chapters.is_empty() {
        Err(err("no chapters found"))
    } else {
        Ok(chapters)
    }
}

fn fetch_viewer_pages(viewer_id: &str) -> ExtensionResult<Vec<MangaPage>> {
    let first = fetch_json(&format!(
        "{API_URL}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id=&page-from=0&page-to=1"
    ))?;
    let total = parse_json(&first)?
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let body = fetch_json(&format!(
        "{API_URL}/book/contentsInfo?comici-viewer-id={viewer_id}&user-id=&page-from=0&page-to={total}"
    ))?;
    let root = parse_json(&body)?;
    let pages = root
        .get("result")
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
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!(
                    "Page {}",
                    item.get("sort").and_then(Value::as_u64).unwrap_or(0) + 1
                )),
                extra,
                ..MangaPage::default()
            })
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        Err(err("no viewer pages found"))
    } else {
        Ok(pages)
    }
}

fn find_viewer_id(body: &str) -> Option<String> {
    let root = serde_json::from_str::<Value>(body).ok()?;
    root.pointer("/episode/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| {
            item.get("type")
                .or_else(|| item.get("kind"))
                .and_then(Value::as_str)
                == Some("viewer")
        })
        .and_then(|item| {
            item.get("viewerId")
                .or_else(|| item.get("viewer_id"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
}

fn item_from_parts(href: &str, chunk: &str) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "series-list-item-h", "</")
            .or_else(|| html::text_between(chunk, "<span", "</span>"))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| json_string_after(chunk, "\"name\":\""))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "RimacomiPlus".into())),
        cover: html::attr_after(chunk, "<img", "src")
            .or_else(|| json_string_after(chunk, "\"src\":\""))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn first_image(value: &Value) -> Option<String> {
    for pointer in ["/image/url", "/thumbnail/url", "/cover", "/imageUrl"] {
        if let Some(image) = value.pointer(pointer).and_then(Value::as_str) {
            return Some(absolute_url(image));
        }
    }
    None
}

fn parse_description(value: &str) -> String {
    html::strip_tags(value).replace("\\n", "\n")
}

fn parse_json(body: &str) -> ExtensionResult<Value> {
    serde_json::from_str(body).map_err(|error| err(&format!("invalid JSON response: {error}")))
}

fn json_string_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\/", "/").replace("\\u002F", "/"))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/series/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn series_hash(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("sample")
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="series-list-item-link" href="/series/sample"><img src="/cover.jpg" alt="Sample RimacomiPlus"><span class="series-list-item-h">Sample RimacomiPlus</span></a>
"#;
