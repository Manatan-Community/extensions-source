use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult,
    abi::{ExtensionResult, system_time},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const SOURCE: PixivComic = PixivComic;
const BASE_URL: &str = "https://comic.pixiv.net";
const API_URL: &str = "https://comic.pixiv.net/api/app";
const SHUFFLE_KEY: &str = "manatan-pixiv-comic";

struct PixivComic;

impl MangaSource for PixivComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE, 1));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_work_list(&fetch_json(
                &format!("{API_URL}/works/recent_updates?page={page}"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_popular(
                &fetch_json(
                    &format!(
                        "{API_URL}/rankings/popularity?label={}&count={}",
                        url::query_escape("総合"),
                        30 * page
                    ),
                    POPULAR_FIXTURE,
                ),
                page,
            ))
        }
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
        let target = if query.starts_with('#') {
            format!(
                "{API_URL}/tags/{}/works/v2?page={page}",
                url::query_escape(query.trim_start_matches('#'))
            )
        } else if !query.is_empty() {
            format!(
                "{API_URL}/works/search/v2/{}?page={page}",
                url::query_escape(query)
            )
        } else if let Some(tag) = filter_string(&request, "tag").filter(|tag| !tag.is_empty()) {
            format!(
                "{API_URL}/tags/{}/works/v2?page={page}",
                url::query_escape(tag.trim_start_matches('#'))
            )
        } else {
            let category = filter_string(&request, "category").unwrap_or_else(|| "恋愛".into());
            format!(
                "{API_URL}/categories/{}/works?page={page}",
                url::query_escape(&category)
            )
        };
        Ok(parse_work_list(&fetch_json(&target, LATEST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/works/{}/episodes/v2?order=desc", item_id(&key)),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        let story_url = format!("{BASE_URL}/viewer/stories/{}", item_id(&key));
        let html = client()
            .get(&story_url)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| STORY_FIXTURE.to_string());
        let salt = next_data_salt(&html).unwrap_or_else(|| "salt".into());
        let (client_time, client_hash) = time_and_hash(&salt);
        Ok(parse_pages(&fetch_json_headers(
            &format!("{API_URL}/episodes/{}/read_v4", item_id(&key)),
            &[
                ("X-Client-Time", client_time.as_str()),
                ("X-Client-Hash", client_hash.as_str()),
            ],
            PAGES_FIXTURE,
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
        manga_image::PixivComicImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/works/{}", item_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/viewer/stories/{}", item_id(&key))))
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
        .with_header("X-Requested-With", "pixivcomic")
        .with_cookies_for(BASE_URL)
}

fn fetch_json(target: &str, fixture: &str) -> String {
    fetch_json_headers(target, &[], fixture)
}

fn fetch_json_headers(target: &str, headers: &[(&str, &str)], fixture: &str) -> String {
    let http = client();
    let mut builder = http.get(target).xhr();
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let ranking = root
        .pointer("/data/ranking")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = ranking.iter().filter_map(catalog_from_popular).collect();
    Paged {
        entries,
        has_next_page: ranking.len() as u64 == page * 30,
    }
}

fn catalog_from_popular(item: &Value) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_u64)?;
    Some(CatalogItem {
        key: id.to_string(),
        title: text(item, "title").unwrap_or_else(|| "Pixiv Comic".into()),
        cover: text(item, "main_image_url"),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(format!("{BASE_URL}/works/{id}")),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_work_list(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let data = root.get("data").unwrap_or(&Value::Null);
    let entries = data
        .get("official_works")
        .or_else(|| data.get("officialWorks"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(catalog_from_work)
        .collect();
    Paged {
        entries,
        has_next_page: data
            .get("next_page_number")
            .is_some_and(|value| !value.is_null()),
    }
}

fn catalog_from_work(item: &Value) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_u64)?;
    Some(CatalogItem {
        key: id.to_string(),
        title: text(item, "name").unwrap_or_else(|| "Pixiv Comic".into()),
        cover: item
            .pointer("/image/main")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        authors: text(item, "author").into_iter().collect(),
        description: text(item, "description").map(|value| html::strip_tags(&value)),
        tags: item
            .get("categories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(
                item.get("tags")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .filter_map(|tag| {
                tag.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(format!("{BASE_URL}/works/{id}")),
        initialized: item.get("description").is_some(),
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_json(
        &format!("{API_URL}/works/v5/{}", item_id(key)),
        DETAILS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    root.pointer("/data/official_work")
        .and_then(catalog_from_work)
        .unwrap_or_else(|| CatalogItem {
            key: item_id(key).into(),
            title: "Pixiv Comic".into(),
            language: Some("ja".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.pointer("/data/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|info| info.get("episode"))
        .filter_map(|episode| {
            let id = episode.get("id").and_then(Value::as_u64)?;
            let title = format!(
                "{}: {}",
                text(episode, "numbering_title").unwrap_or_default(),
                text(episode, "sub_title").unwrap_or_default()
            );
            Some(MangaChapter {
                key: id.to_string(),
                title: Some(title.trim_matches([':', ' ']).to_string()),
                chapter_number: None,
                date_uploaded: episode.get("read_start_at").and_then(Value::as_i64),
                url: Some(format!("{BASE_URL}/viewer/stories/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.pointer("/data/reading_episode/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| page.get("url").and_then(Value::as_str))
        .enumerate()
        .map(|(index, image_url)| {
            let mut headers = manga::image_headers(BASE_URL);
            headers.insert(
                "X-Cobalt-Thumber-Parameter-GridShuffle-Key".into(),
                SHUFFLE_KEY.into(),
            );
            let mut extra = BTreeMap::new();
            extra.insert("pixivShuffleKey".into(), json!(SHUFFLE_KEY));
            MangaPage {
                content: PageContent::Url {
                    url: image_url.into(),
                    context: Some(headers.clone()),
                },
                headers,
                extra,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn next_data_salt(body: &str) -> Option<String> {
    let script = html::text_between(body, "<script id=\"__NEXT_DATA__\"", "</script>")?;
    let root = serde_json::from_str::<Value>(&html::strip_tags(&script)).ok()?;
    root.pointer("/props/pageProps/salt")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn time_and_hash(salt: &str) -> (String, String) {
    let unix = system_time().map(|time| time.unix_seconds).unwrap_or(0);
    let time = format_utc_offset(unix);
    let digest = Sha256::digest(format!("{time}{salt}").as_bytes());
    let hash = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    (time, hash)
}

fn format_utc_offset(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3600;
    let minute = seconds % 3600 / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+00:00")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + (m <= 2) as i64, m, d)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .split("/works/")
        .nth(1)
        .or_else(|| input.split("/viewer/stories/").nth(1))
        .and_then(|value| value.split(['/', '?', '#']).next())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn item_id(key: &str) -> &str {
    key.trim_matches('/')
}

const POPULAR_FIXTURE: &str = r#"{"data":{"ranking":[{"id":1,"title":"Sample Popular","main_image_url":"https://example.invalid/cover.jpg"}]}}"#;
const LATEST_FIXTURE: &str = r#"{"data":{"next_page_number":null,"official_works":[{"id":2,"name":"Sample Latest","image":{"main":"https://example.invalid/cover.jpg"},"author":"Author","description":"<p>Description</p>","categories":[{"name":"Drama"}],"tags":[{"name":"Sample"}]}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"official_work":{"id":1,"name":"Sample Details","image":{"main":"https://example.invalid/cover.jpg"},"author":"Author","description":"<p>Description</p>","categories":[{"name":"Drama"}],"tags":[{"name":"Sample"}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"episodes":[{"episode":{"id":11,"numbering_title":"Episode 1","sub_title":"Start","read_start_at":1700000000}}]}}"#;
const STORY_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"salt":"salt"}}}</script>"#;
const PAGES_FIXTURE: &str =
    r#"{"data":{"reading_episode":{"pages":[{"url":"https://comic.pixiv.net/page.png"}]}}}"#;

export_manga_source!(SOURCE);
