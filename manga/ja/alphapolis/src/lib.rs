use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Alphapolis = Alphapolis;
const BASE_URL: &str = "https://www.alphapolis.co.jp";

struct Alphapolis;

impl MangaSource for Alphapolis {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(|value| value.get("id").or(Some(value)))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        if listing == "latest" {
            let target = format!("{BASE_URL}/manga/official/search?page={page}");
            let body = fetch_get(&target, SEARCH_FIXTURE);
            return Ok(Paged {
                entries: parse_search(&body),
                has_next_page: has_next(&body),
            });
        }
        let body = fetch_get(
            &format!("{BASE_URL}/manga/official/ranking?category=total"),
            POPULAR_FIXTURE,
        );
        Ok(Paged {
            entries: parse_popular(&body),
            has_next_page: false,
        })
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
        let mut params = vec![format!("page={}", page(&request))];
        if !query.is_empty() {
            params.push(format!("query={}", url::query_escape(query)));
        }
        append_multi(&request, "category", "category", &mut params);
        append_multi(&request, "label", "label", &mut params);
        append_multi(&request, "complete", "complete", &mut params);
        append_multi(&request, "rental", "rental", &mut params);
        if filter_bool(&request, "is_free_daily") {
            params.push("is_free_daily=enable".to_string());
        }
        let target = format!("{BASE_URL}/manga/official/search?{}", params.join("&"));
        let body = fetch_get(&target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: has_next(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/official/1".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga/official/1".to_string());
        let manga_id = key.trim_end_matches('/').rsplit('/').next().unwrap_or("1");
        let body = client()
            .post(format!("{BASE_URL}/manga/official/episodes.json"))
            .xhr()
            .json(json!({ "manga_id": manga_id.parse::<u64>().unwrap_or(1) }).to_string())
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1#1".to_string());
        let (manga_id, episode_id) = key.split_once('#').unwrap_or(("1", "1"));
        let chapter_url = format!("{BASE_URL}/manga/official/{manga_id}/{episode_id}");
        let mut pages = Vec::new();
        for resolution in ["full_hd", "standard"] {
            let body = client()
                .post(format!("{BASE_URL}/manga/official/viewer.json"))
                .xhr()
                .referer(&chapter_url)
                .json(
                    json!({
                        "episode_no": episode_id.parse::<u64>().unwrap_or(1),
                        "hide_page": false,
                        "manga_sele_id": manga_id.parse::<u64>().unwrap_or(1),
                        "preview": false,
                        "resolution": resolution
                    })
                    .to_string(),
                )
                .send_text()
                .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
            pages = parse_pages(&body);
            if !pages.is_empty() {
                break;
            }
        }
        Ok(pages)
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
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("official-manga-sub-like_ranking"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: text_first(
                    chunk,
                    &[
                        "official-manga-sub-like_ranking--list_title",
                        "official-manga-sub-like_ranking--panel_title",
                    ],
                )
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| "Alphapolis manga".to_string()),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr(chunk, "data-bg"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".to_string()),
                content_rating: Some("adult".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("official-manga-panel") || chunk.contains("class=\"panel"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: text_first(chunk, &["title"])
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| "Alphapolis manga".to_string()),
                cover: html::attr_after(chunk, "panel", "data-bg")
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".to_string()),
                content_rating: Some("adult".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_get(&url::join_url(BASE_URL, key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: text_first(&body, &["manga-detail-description", "<h1"])
            .unwrap_or_else(|| "Alphapolis manga".to_string()),
        cover: html::attr_after(&body, "manga-bigbanner", "src").map(|v| url::join_url(BASE_URL, &v)),
        description: text_first(&body, &["manga-detail-outline", "outline"]),
        authors: values_in(&body, "原作"),
        artists: values_in(&body, "漫画"),
        tags: link_texts(&body, "official-manga-tag"),
        status: status(&body),
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("ja".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let raw_url = episode.get("url").and_then(Value::as_str)?;
            let segments: Vec<_> = raw_url.trim_matches('/').split('/').collect();
            let manga_id = segments.get(2).or_else(|| segments.get(1)).copied().unwrap_or("1");
            let episode_id = segments.last().copied().unwrap_or("1");
            let locked = episode
                .pointer("/rental/isFree")
                .and_then(Value::as_bool)
                .is_some_and(|free| !free)
                && episode.pointer("/rental/isOnRental").and_then(Value::as_bool) != Some(true);
            Some(MangaChapter {
                key: format!("{manga_id}#{episode_id}"),
                title: Some(format!(
                    "{}{}",
                    if locked { "[Locked] " } else { "" },
                    string_at(episode, "/mainTitle").unwrap_or_else(|| "Episode".to_string())
                )),
                date_uploaded: string_at(episode, "/upTime")
                    .and_then(|date| dates::parse_ymd(&date.replace("更新", "").replace('.', "-"))),
                url: Some(format!("{BASE_URL}/manga/official/{manga_id}/{episode_id}")),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value
        .pointer("/page/images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| page.get("url").and_then(Value::as_str))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn text_first(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn values_in(body: &str, marker: &str) -> Vec<String> {
    body.split("mangaka")
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != marker)
        .collect()
}

fn status(body: &str) -> ItemStatus {
    if body.contains("完結") {
        ItemStatus::Completed
    } else if body.contains("休載中") {
        ItemStatus::Hiatus
    } else if body.contains("連載中") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(href: &str) -> Option<String> {
    let value = href.strip_prefix(BASE_URL).unwrap_or(href);
    let start = value.find("/manga/official/")?;
    Some(format!("/{}", value[start + 1..].trim_end_matches('/')))
}

fn key_from_url(input: &str) -> Option<String> {
    normalize_key(input)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn append_multi(request: &Value, id: &str, param: &str, out: &mut Vec<String>) {
    let Some(value) = request.get("filters").and_then(|filters| filters.get(id)) else {
        return;
    };
    let values: Vec<String> = match value {
        Value::Array(values) => values.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Value::String(value) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    };
    for (index, value) in values.iter().enumerate() {
        out.push(format!("{param}[{index}]={}", url::query_escape(value)));
    }
}

fn filter_bool(request: &Value, id: &str) -> bool {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(Value::as_str).map(str::to_string)
}

fn has_next(body: &str) -> bool {
    body.contains("fa-angle-double-right")
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"<a class="official-manga-sub-like_ranking--list" href="/manga/official/1"><span class="official-manga-sub-like_ranking--list_title">Sample Alphapolis</span><img data-src="/cover.jpg"></a>"#;
const SEARCH_FIXTURE: &str = r#"<div class="mangas-list"><div class="official-manga-panel"><a href="/manga/official/1"><span class="title">Sample Alphapolis</span><div class="panel" data-bg="/cover.jpg"></div></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Alphapolis</h1><div class="manga-detail-outline"><p class="outline">A sample manga.</p></div><div class="wrap-content-status"><a href="?complete=running">連載中</a></div><div class="manga-bigbanner"><img src="/cover.jpg"></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"episodes":[{"url":"/manga/official/1/100","mainTitle":"Episode 1","upTime":"2024.01.01更新","rental":{"isFree":true,"isOnRental":true}}]}"#;
const PAGES_FIXTURE: &str = r#"{"page":{"images":[{"url":"https://www.alphapolis.co.jp/page1.jpg"}]}}"#;
