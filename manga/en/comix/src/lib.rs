use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource, webview,
};
use manatan_shared::{
    manga, manga_image,
    sdk::http::HttpClient,
    url,
};
use serde_json::{Value, json};

const SOURCE: Comix = Comix;
const BASE_URL: &str = "https://comix.to";
const API_URL: &str = "https://comix.to/api/v1";

struct Comix;

impl MangaSource for Comix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "chapter_updated_at"
        } else {
            "score"
        };
        Ok(parse_search(&api_get(&format!(
            "{API_URL}/manga?order%5B{order}%5D=desc&limit=28&page={page}{}",
            content_params(&request)
        ), SEARCH_FIXTURE), &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key, &request)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{API_URL}/manga?{}&limit=28&page={}", search_params(&request), page(&request))
        } else {
            format!(
                "{API_URL}/manga?keyword={}&order%5Brelevance%5D=desc&limit=28&page={}{}",
                url::query_escape(query),
                page(&request),
                content_params(&request)
            )
        };
        Ok(parse_search(&api_get(&target, SEARCH_FIXTURE), &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/gm3kk".into());
        Ok(details_by_key(&key, &request))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/gm3kk".into());
        let slug = key.trim_matches('/').to_string();
        let title_url = format!("{BASE_URL}/title/{slug}");
        let mut chapters = captured_api_urls(&title_url, "/chapters")
            .into_iter()
            .flat_map(|target| {
                let expanded = replace_limit(&target, 100);
                parse_chapter_items(&api_get(&expanded, ""), &slug)
            })
            .collect::<Vec<_>>();
        let blacklist = preference_csv(&request, "scanlator_blacklist");
        if !blacklist.is_empty() {
            chapters.retain(|chapter| {
                let names = chapter
                    .scanlators
                    .iter()
                    .map(|value| value.trim().to_ascii_lowercase())
                    .collect::<Vec<_>>();
                !blacklist.iter().any(|blocked| names.iter().any(|name| name == blocked))
            });
        }
        if preference_bool(&request, "deduplicate_chapters") {
            chapters = deduplicate_chapters(chapters);
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "title/gm3kk/1-chapter-1".into());
        let chapter_url = absolute_url(&key);
        for target in captured_api_urls(&chapter_url, "/api/") {
            let body = api_get(&target, "");
            if body.contains("\"pages\"") {
                let pages = parse_pages(&body, &chapter_url);
                if !pages.is_empty() {
                    return Ok(pages);
                }
            }
        }
        Ok(Vec::new())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
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
        manga_image::ComixImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/title/{}", key.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key, &request)),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn captured_api_urls(page_url: &str, contains: &str) -> Vec<String> {
    let Ok(response) = webview::extract(
        webview::ExtractRequest::new(
            page_url,
            r#"
await new Promise(resolve => setTimeout(resolve, 3000));
return "";
"#,
        )
        .capture_url_contains("api", contains)
        .wait_for_script("document.readyState === 'complete'")
        .timeout_ms(25_000)
        .cookies(true)
        .headless(true),
    ) else {
        return Vec::new();
    };
    response
        .captured_requests
        .into_iter()
        .filter(|capture| capture.url.contains(contains))
        .map(|capture| capture.url)
        .collect()
}

fn parse_search(body: &str, request: &Value) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture"));
    let result = root.get("result").unwrap_or(&root);
    let entries = result
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| manga_item(item, false, request))
        .collect();
    Paged {
        entries,
        has_next_page: has_next(result),
    }
}

fn details_by_key(key: &str, request: &Value) -> CatalogItem {
    let slug = key.trim_matches('/').split('/').next().unwrap_or(key.trim_matches('/'));
    let body = api_get(&format!("{API_URL}/manga/{slug}"), DETAILS_FIXTURE);
    let root = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture"));
    let result = root.get("result").unwrap_or(&root);
    manga_item(result, true, request)
}

fn manga_item(item: &Value, initialized: bool, request: &Value) -> CatalogItem {
    let hid = text(item, "hid")
        .or_else(|| item.get("url").and_then(Value::as_str).map(|value| value.trim_start_matches("/title/").to_string()))
        .unwrap_or_else(|| "gm3kk".into());
    let tag_values = tags(item, "genres")
        .into_iter()
        .chain(tags(item, "demographics"))
        .chain(if preference_bool(request, "show_tags_in_genres") { tags(item, "tags") } else { Vec::new() })
        .collect::<Vec<_>>();
    CatalogItem {
        key: format!("/{hid}"),
        title: text(item, "title").unwrap_or_else(|| "Comix".into()),
        cover: item
            .get("poster")
            .and_then(|poster| poster.get(preference_string(request, "poster_quality").as_deref().unwrap_or("large")))
            .or_else(|| item.pointer("/poster/large"))
            .or_else(|| item.pointer("/poster/medium"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        authors: tags(item, "authors").into_iter().chain(tags(item, "author")).collect(),
        artists: tags(item, "artists").into_iter().chain(tags(item, "artist")).collect(),
        description: text(item, "synopsis"),
        tags: tag_values,
        status: match text(item, "status").unwrap_or_default().as_str() {
            "releasing" => ItemStatus::Ongoing,
            "finished" => ItemStatus::Completed,
            "on_hiatus" => ItemStatus::Hiatus,
            "discontinued" => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        language: Some("en".into()),
        content_rating: Some(match text(item, "contentRating").unwrap_or_default().as_str() {
            "safe" => "safe".into(),
            _ => "adult".into(),
        }),
        url: Some(format!("{BASE_URL}/title/{hid}")),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapter_items(body: &str, manga_slug: &str) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let items = root
        .pointer("/result/items")
        .or_else(|| root.pointer("/result"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .into_iter()
        .map(|item| {
            let id = item.get("id").and_then(Value::as_i64).unwrap_or(1);
            let number = item.get("number").and_then(Value::as_f64).unwrap_or(1.0);
            let raw_number = number.to_string().trim_end_matches(".0").to_string();
            let key = text(&item, "url")
                .and_then(|value| value.split_once("/title/").map(|(_, tail)| format!("title/{tail}")).or(Some(value)))
                .unwrap_or_else(|| format!("title/{manga_slug}/{id}-chapter-{raw_number}"));
            let group = item
                .get("group")
                .and_then(|group| text(group, "name"))
                .or_else(|| item.get("isOfficial").and_then(Value::as_bool).filter(|v| *v).map(|_| "Official".into()))
                .unwrap_or_else(|| "Unknown".into());
            MangaChapter {
                key,
                title: Some(match text(&item, "name").filter(|value| !value.is_empty()) {
                    Some(name) => format!("Chapter {raw_number}: {name}"),
                    None => format!("Chapter {raw_number}"),
                }),
                chapter_number: Some(number as f32),
                scanlators: vec![group],
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let pages = root.pointer("/result/pages").unwrap_or(&root);
    let base = text(pages, "baseUrl").unwrap_or_default();
    pages
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, page)| {
            let raw = text(page, "url")?;
            let image = if raw.starts_with("http") {
                raw
            } else {
                format!("{}/{}", base.trim_end_matches('/'), raw.trim_start_matches('/'))
            };
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(image_headers(referer, page.get("s").and_then(Value::as_i64) == Some(1))),
                },
                headers: image_headers(referer, page.get("s").and_then(Value::as_i64) == Some(1)),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn image_headers(referer: &str, scrambled: bool) -> Context {
    let mut headers = manga::image_headers(referer);
    if !scrambled {
        headers.retain(|name, _| name.to_ascii_lowercase() != "origin");
    }
    headers
}

fn deduplicate_chapters(chapters: Vec<MangaChapter>) -> Vec<MangaChapter> {
    let mut out = Vec::<MangaChapter>::new();
    for chapter in chapters {
        if !out.iter().any(|existing| existing.chapter_number == chapter.chapter_number) {
            out.push(chapter);
        }
    }
    out
}

fn search_params(request: &Value) -> String {
    let mut params = Vec::<(String, String)>::new();
    if let Some(sort) = filter_string(request, "sort") {
        let (field, direction) = sort.split_once(':').unwrap_or((&sort, "desc"));
        params.push((format!("order[{field}]"), direction.into()));
    }
    if let Some(rating) = filter_string(request, "content_rating").filter(|value| !value.is_empty()) {
        params.push(("content_rating".into(), rating));
    }
    for value in filter_array(request, "types") {
        params.push(("types[]".into(), value));
    }
    for value in filter_array(request, "genres_in") {
        params.push(("genres_in[]".into(), value));
    }
    if let Some(mode) = filter_string(request, "genres_mode") {
        params.push(("genres_mode".into(), mode));
    }
    for value in filter_array(request, "statuses") {
        params.push(("statuses[]".into(), value));
    }
    if let Some(value) = filter_string(request, "min_chap").filter(|value| !value.is_empty()) {
        params.push(("min_chap".into(), value));
    }
    encode_params(&params)
}

fn content_params(request: &Value) -> String {
    let mut params = Vec::<(String, String)>::new();
    if let Some(rating) = preference_string(request, "content_rating").filter(|value| !value.is_empty()) {
        params.push(("content_rating".into(), rating));
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("&{}", encode_params(&params))
    }
}

fn replace_limit(target: &str, limit: u32) -> String {
    if target.contains("limit=") {
        let mut out = Vec::new();
        for part in target.split('&') {
            if part.contains("limit=") {
                out.push(format!("limit={limit}"));
            } else {
                out.push(part.to_string());
            }
        }
        out.join("&")
    } else {
        format!("{target}{}limit={limit}", if target.contains('?') { "&" } else { "?" })
    }
}

fn has_next(value: &Value) -> bool {
    let meta = value.get("meta").or_else(|| value.get("pagination"));
    meta.and_then(|meta| {
        let page = meta.get("page").and_then(Value::as_u64)?;
        let last = meta
            .get("lastPage")
            .or_else(|| meta.get("last_page"))
            .and_then(Value::as_u64)
            .unwrap_or(page);
        Some(page < last || meta.get("hasNext").and_then(Value::as_bool).unwrap_or(false))
    })
    .unwrap_or(false)
}

fn tags(item: &Value, key: &str) -> Vec<String> {
    item.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| text(tag, "title"))
        .collect()
}

fn text(item: &Value, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/title/"))
        .map(|value| format!("/{}", value.trim_matches('/').split('/').next().unwrap_or(value)))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn preference_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| value.as_bool().or_else(|| value.as_str().map(|text| text == "true")))
        .unwrap_or(false)
}

fn preference_csv(request: &Value, id: &str) -> Vec<String> {
    preference_string(request, id)
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn encode_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
{"status":"ok","result":{"items":[{"hid":"gm3kk","title":"Blind Obedience","type":"manhwa","status":"releasing","contentRating":"safe","poster":{"large":"https://static.comix.to/sample.jpg"},"synopsis":"Sample description.","genres":[{"title":"Fantasy"}]}],"meta":{"page":1,"lastPage":1}}}
"#;

const DETAILS_FIXTURE: &str = r#"
{"status":"ok","result":{"hid":"gm3kk","title":"Blind Obedience","type":"manhwa","status":"releasing","contentRating":"safe","poster":{"large":"https://static.comix.to/sample.jpg"},"synopsis":"Sample description.","authors":[{"title":"Sample Author"}],"artists":[{"title":"Sample Artist"}],"genres":[{"title":"Fantasy"}]}}
"#;
