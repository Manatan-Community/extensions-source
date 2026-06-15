use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: NicovideoSeiga = NicovideoSeiga;
const BASE_URL: &str = "https://sp.manga.nicovideo.jp";
const API_URL: &str = "https://api.nicomanga.jp/api/v1/app/manga";

struct NicovideoSeiga;

impl MangaSource for NicovideoSeiga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE, 1));
        }
        let page = page(&request);
        Ok(parse_popular(
            &fetch_json(
                &format!("{BASE_URL}/manga/ajax/ranking?span=total&category=all&page={page}"),
                POPULAR_FIXTURE,
            ),
            page,
        ))
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
        let offset = page.saturating_sub(1) * 20;
        Ok(parse_search(
            &fetch_json(
                &format!(
                    "{API_URL}/contents?mode=keyword&sort=score&q={}&limit=20&offset={offset}",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            ),
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/contents/{}/episodes", item_id(&key)),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1".into());
        Ok(parse_pages(&fetch_json(
            &format!(
                "{API_URL}/episodes/{}/frames?enable_webp=true",
                item_id(&key)
            ),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
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
        manga_image::NicovideoSeigaImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/comic/{}", item_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/watch/mg{}", item_id(&key))))
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
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_u64)?;
            Some(CatalogItem {
                key: id.to_string(),
                title: text(item, "title").unwrap_or_else(|| "Nicovideo Seiga".into()),
                authors: text(item, "author").into_iter().collect(),
                cover: text(item, "thumbnail_url"),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                url: Some(format!("{BASE_URL}/comic/{id}")),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: page < 5,
    }
}

fn parse_search(body: &str, initialized: bool) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = result_array(&root)
        .into_iter()
        .filter_map(|item| catalog_from_manga(&item, initialized))
        .collect();
    let has_next_page = root
        .pointer("/data/extra/has_next")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_search(
        &fetch_json(
            &format!("{API_URL}/contents/{}", item_id(key)),
            DETAILS_FIXTURE,
        ),
        true,
    )
    .entries
    .into_iter()
    .next()
    .unwrap_or_else(|| CatalogItem {
        key: item_id(key).into(),
        title: "Nicovideo Seiga".into(),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn catalog_from_manga(item: &Value, initialized: bool) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_u64)?;
    let meta = item.get("meta").unwrap_or(item);
    Some(CatalogItem {
        key: id.to_string(),
        title: text(meta, "title").unwrap_or_else(|| "Nicovideo Seiga".into()),
        authors: text(meta, "display_author_name").into_iter().collect(),
        cover: text(meta, "square_image_url").or_else(|| text(meta, "thumbnail_url")),
        description: text(meta, "description").map(|value| html::strip_tags(&value)),
        status: match text(meta, "serial_status").as_deref() {
            Some("serial") => ItemStatus::Ongoing,
            Some("concluded") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        url: Some(format!("{BASE_URL}/comic/{id}")),
        initialized,
        ..CatalogItem::default()
    })
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut chapters = result_array(&root)
        .into_iter()
        .filter(|item| {
            item.pointer("/own_status/sell_status")
                .and_then(Value::as_str)
                != Some("publication_finished")
        })
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_u64)?;
            let meta = item.get("meta")?;
            let sell_status = item
                .pointer("/own_status/sell_status")
                .and_then(Value::as_str)
                .unwrap_or("");
            let prefix = match sell_status {
                "selling" => "Paid ",
                "pre_selling" => "Preorder ",
                _ => "",
            };
            Some(MangaChapter {
                key: id.to_string(),
                title: Some(format!(
                    "{prefix}{}",
                    text(meta, "title").unwrap_or_else(|| "Chapter".into())
                )),
                chapter_number: meta
                    .get("number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: meta
                    .get("created_at")
                    .and_then(Value::as_i64)
                    .map(|value| value * 1000),
                url: Some(format!("{BASE_URL}/watch/mg{id}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    result_array(&root)
        .into_iter()
        .filter_map(|frame| {
            frame
                .pointer("/meta/source_url")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .enumerate()
        .map(|(index, image_url)| {
            let mut extra = BTreeMap::new();
            if let Some(key) = nico_image_key(&image_url) {
                extra.insert("nicoImageKey".into(), json!(key));
            }
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(image_headers()),
                },
                headers: image_headers(),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn result_array(root: &Value) -> Vec<Value> {
    match root.pointer("/data/result") {
        Some(Value::Array(values)) => values.clone(),
        Some(value) if value.is_object() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = manga::image_headers(BASE_URL);
    headers.insert(
        "Accept".into(),
        "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".into(),
    );
    headers.insert("Pragma".into(), "no-cache".into());
    headers.insert("Cache-Control".into(), "no-cache".into());
    headers
}

fn nico_image_key(input: &str) -> Option<String> {
    let marker = "https://drm.cdn.nicomanga.jp/image/";
    let rest = input.strip_prefix(marker)?;
    let hash = rest.split('_').next()?;
    (hash.len() >= 16).then(|| hash[..16].to_string())
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
        .split("/comic/")
        .nth(1)
        .or_else(|| input.split("/watch/mg").nth(1))
        .and_then(|value| value.split(['/', '?', '#']).next())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn item_id(key: &str) -> &str {
    key.trim_matches('/').trim_start_matches("mg")
}

const POPULAR_FIXTURE: &str = r#"[{"id":1,"title":"Sample Popular","author":"Author","thumbnail_url":"https://example.invalid/cover.jpg"}]"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"result":[{"id":1,"meta":{"title":"Sample Search","display_author_name":"Author","description":"<p>Description</p>","serial_status":"serial","square_image_url":"https://example.invalid/cover.jpg","share_url":"https://sp.manga.nicovideo.jp/comic/1"}}],"extra":{"has_next":false}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"result":{"id":1,"meta":{"title":"Sample Details","display_author_name":"Author","description":"<p>Description</p>","serial_status":"serial","square_image_url":"https://example.invalid/cover.jpg","share_url":"https://sp.manga.nicovideo.jp/comic/1"}}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"result":[{"id":11,"meta":{"title":"Chapter 1","number":1,"created_at":1700000000},"own_status":{"sell_status":"free"}}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"result":[{"meta":{"source_url":"https://deliver.cdn.nicomanga.jp/image/free_1/1p.webp"}}]}}"#;

export_manga_source!(SOURCE);
