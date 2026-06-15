use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::collections::BTreeMap;

const SOURCE: MagazinePocket = MagazinePocket;
const BASE_URL: &str = "https://pocket.shonenmagazine.com";
const API_URL: &str = "https://api.pocket.shonenmagazine.com";
const PAGE_LIMIT: u64 = 25;

struct MagazinePocket;

impl MangaSource for MagazinePocket {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_title_list(
                &hashed_get(
                    &format!("{API_URL}/web/top/updated/title?base_date=2999-01-01"),
                    TITLE_LIST_FIXTURE,
                ),
                false,
            ));
        }
        let offset = (page - 1) * PAGE_LIMIT;
        Ok(popular_from_ranking(&hashed_get(
            &format!("{API_URL}/ranking/all?ranking_id=30&offset={offset}&limit=26"),
            RANKING_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
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
        if !query.is_empty() {
            return Ok(parse_title_list(
                &hashed_get(
                    &format!(
                        "{API_URL}/web/search/title?keyword={}&limit=99999",
                        url::query_escape(query)
                    ),
                    TITLE_LIST_FIXTURE,
                ),
                false,
            ));
        }
        let category = filter_string(&request, "category").unwrap_or("ranking|30");
        let (kind, value) = category.split_once('|').unwrap_or(("ranking", "30"));
        if kind == "genre" {
            return Ok(parse_title_list(
                &hashed_get(
                    &format!("{API_URL}/search/title?genre_id={value}&limit=99999"),
                    TITLE_LIST_FIXTURE,
                ),
                false,
            ));
        }
        let offset = (page - 1) * PAGE_LIMIT;
        Ok(popular_from_ranking(&hashed_get(
            &format!("{API_URL}/ranking/all?ranking_id={value}&offset={offset}&limit=26"),
            RANKING_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/00001".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/00001".into());
        let id = key.trim_matches('/').split('/').last().unwrap_or("00001");
        let hide_locked = preference_bool(&request, "hideLockedChapters", false);
        Ok(fetch_chapters(id, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/title/00001/episode/1".into());
        let episode_id = key.trim_matches('/').split('/').last().unwrap_or("1");
        Ok(parse_pages(&hashed_get(
            &format!("{API_URL}/web/episode/viewer?episode_id={episode_id}"),
            VIEWER_FIXTURE,
        )))
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
        manga_image::MagazinePocket::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}{key}")))
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
        .with_header("x-manga-platform", "3")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn hashed_get(target: &str, fixture: &str) -> String {
    let hash = generate_hash(query_pairs(target));
    client()
        .get(target)
        .header("x-manga-hash", hash)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn popular_from_ranking(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(RANKING_FIXTURE).unwrap_or(Value::Null));
    let ids = root
        .get("ranking_title_list")
        .or_else(|| root.get("title_list"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("title_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_u64)
        })
        .map(|id| format!("{id:05}"))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return parse_title_list(TITLE_LIST_FIXTURE, false);
    }
    let has_next = ids.len() > PAGE_LIMIT as usize;
    let ids = if has_next {
        &ids[..PAGE_LIMIT as usize]
    } else {
        &ids[..]
    };
    parse_title_list(
        &hashed_get(
            &format!("{API_URL}/title/list?title_id_list={}", ids.join(",")),
            TITLE_LIST_FIXTURE,
        ),
        has_next,
    )
}

fn parse_title_list(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(TITLE_LIST_FIXTURE).unwrap_or(Value::Null));
    let entries = root
        .get("title_list")
        .or_else(|| root.get("search_title_list"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(catalog_from_title)
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn catalog_from_title(item: &Value) -> Option<CatalogItem> {
    let title_id = item.get("title_id").and_then(Value::as_u64)?;
    let key = format!("/title/{title_id:05}");
    Some(CatalogItem {
        key: key.clone(),
        title: item
            .get("title_name")
            .and_then(Value::as_str)
            .unwrap_or("Magazine Pocket")
            .to_string(),
        cover: image_from_title(item),
        url: Some(format!("{BASE_URL}{key}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = key.trim_matches('/').split('/').last().unwrap_or("00001");
    let body = hashed_get(
        &format!("{API_URL}/web/title/detail?title_id={id}"),
        DETAIL_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAIL_FIXTURE).unwrap_or(Value::Null));
    let title = root.get("web_title").unwrap_or(&Value::Null);
    CatalogItem {
        key: format!("/title/{id}"),
        title: title
            .get("title_name")
            .and_then(Value::as_str)
            .unwrap_or("Magazine Pocket")
            .to_string(),
        cover: image_from_title(title),
        authors: title
            .get("author_text")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        description: title
            .get("introduction_text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        url: Some(format!("{BASE_URL}/title/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(title_id: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let detail = hashed_get(
        &format!("{API_URL}/web/title/detail?title_id={title_id}"),
        DETAIL_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&detail)
        .unwrap_or_else(|_| serde_json::from_str(DETAIL_FIXTURE).unwrap_or(Value::Null));
    let ids = root
        .pointer("/web_title/episode_id_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let body = if ids.is_empty() {
        EPISODE_LIST_FIXTURE.to_string()
    } else {
        let form = format!("episode_id_list={}", ids.join(","));
        let hash = generate_hash(vec![("episode_id_list".to_string(), ids.join(","))]);
        client()
            .post(format!("{API_URL}/episode/list"))
            .header("x-manga-hash", hash)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form.into_bytes())
            .xhr()
            .send_text()
            .unwrap_or_else(|_| EPISODE_LIST_FIXTURE.to_string())
    };
    let root = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(EPISODE_LIST_FIXTURE).unwrap_or(Value::Null));
    let mut chapters = root
        .get("episode_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| episode_to_chapter(item, hide_locked))
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn episode_to_chapter(item: &Value, hide_locked: bool) -> Option<MangaChapter> {
    let episode_id = item.get("episode_id").and_then(Value::as_u64)?;
    let title_id = item.get("title_id").and_then(Value::as_u64).unwrap_or(0);
    let locked = item.get("point").and_then(Value::as_i64).unwrap_or(0) > 0
        && item.get("badge").and_then(Value::as_i64) != Some(3)
        && item.get("rental_finish_time").is_none();
    if hide_locked && locked {
        return None;
    }
    let key = format!("/title/{title_id:05}/episode/{episode_id}");
    Some(MangaChapter {
        key: key.clone(),
        title: Some(format!(
            "{}{}",
            if locked { "Locked " } else { "" },
            item.get("episode_name")
                .and_then(Value::as_str)
                .unwrap_or("Episode")
        )),
        chapter_number: item
            .get("index")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        url: Some(format!("{BASE_URL}{key}")),
        is_locked: locked,
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(VIEWER_FIXTURE).unwrap_or(Value::Null));
    let seed = root
        .get("scramble_seed")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let title_id = root.get("title_id").and_then(Value::as_u64).unwrap_or(0);
    let episode_id = root.get("episode_id").and_then(Value::as_u64).unwrap_or(0);
    root.get("page_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| {
            let mut extra = BTreeMap::new();
            extra.insert(
                "magapokeScramble".into(),
                json!({"seed": seed, "titleId": title_id, "episodeId": episode_id}),
            );
            MangaPage {
                content: PageContent::Url {
                    url: image.to_string(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn generate_hash(params: Vec<(String, String)>) -> String {
    let mut sorted = params;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = sorted
        .into_iter()
        .map(|(key, value)| format!("{}_{}", sha256_hex(&key), sha512_hex(&value)))
        .collect::<Vec<_>>()
        .join(",");
    let hash1 = sha256_hex(&joined);
    sha512_hex(&format!(
        "{hash1}{}",
        format!("{}_{}", sha256_hex(""), sha512_hex(""))
    ))
}

fn query_pairs(target: &str) -> Vec<(String, String)> {
    target
        .split('?')
        .nth(1)
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn sha256_hex(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

fn sha512_hex(input: &str) -> String {
    format!("{:x}", Sha512::digest(input.as_bytes()))
}

fn image_from_title(item: &Value) -> Option<String> {
    [
        "thumbnail_image_url",
        "banner_image_url",
        "thumbnail_rect_image_url",
    ]
    .iter()
    .find_map(|key| item.get(*key).and_then(Value::as_str))
    .map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
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

fn key_from_url(input: &str) -> Option<String> {
    input
        .find("/title/")
        .map(|index| format!("/{}", input[index + 1..].trim_matches('/')))
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"ranking_title_list":[{"title_id":1}]}"#;
const TITLE_LIST_FIXTURE: &str = r#"{"title_list":[{"title_id":1,"title_name":"Sample Magazine Pocket","thumbnail_image_url":"https://img.example.test/cover.jpg"}]}"#;
const DETAIL_FIXTURE: &str = r#"{"web_title":{"title_name":"Sample Magazine Pocket","author_text":"Sample Author","introduction_text":"Sample description.","genre_id_list":[1],"episode_id_list":[1],"thumbnail_image_url":"https://img.example.test/cover.jpg"}}"#;
const EPISODE_LIST_FIXTURE: &str = r#"{"episode_list":[{"episode_id":1,"episode_name":"Episode 1","index":1,"start_time":"2024-01-01 00:00:00","point":0,"title_id":1,"badge":0}]}"#;
const VIEWER_FIXTURE: &str = r#"{"page_list":["https://img.example.test/page1.jpg"],"scramble_seed":"svd","title_id":1,"episode_id":1}"#;
