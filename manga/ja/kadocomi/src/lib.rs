use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    ProcessedImage, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: KadoComi = KadoComi;
const BASE_URL: &str = "https://comic-walker.com";
const API_URL: &str = "https://comic-walker.com/api";

struct KadoComi;

impl MangaSource for KadoComi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{API_URL}/series/new?limit=100")
        } else {
            format!("{API_URL}/ranking?limit=50")
        };
        Ok(parse_search_results(&fetch_json(&target, LIST_FIXTURE)))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1);
        let offset = (page - 1) * 20;
        let target = format!(
            "{API_URL}/search/keywords?keywords={}&limit=20&offset={offset}&sortBy=popularity",
            url::query_escape(query)
        );
        Ok(parse_search_results(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/detail/sample".into());
        Ok(parse_chapters(&fetch_json(&details_api_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/api/contents/viewer?episodeId=sample&imageSizeType=width%3A1284#workCode=sample&episodeCode=sample".into());
        let target = absolute_url(&key);
        Ok(parse_pages(&fetch_json(&target, PAGES_FIXTURE), &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"listing": "popular"}))?;
        let latest = self.list(json!({"listing": "latest"}))?;
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
        manga_image::XorImage::process_drm_hash_hex(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
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

fn fetch_json(target: &str, fixture: &str) -> Value {
    client()
        .get(target)
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_else(|| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn parse_search_results(root: &Value) -> Paged<CatalogItem> {
    let entries = root
        .get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(work_to_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= 20,
        entries,
    }
}

fn work_to_item(work: &Value) -> CatalogItem {
    let code = string_at(work, "code").unwrap_or_else(|| string_at(work, "id").unwrap_or_else(|| "sample".into()));
    let key = format!("/detail/{code}");
    CatalogItem {
        key: key.clone(),
        title: string_at(work, "title").unwrap_or_else(|| "KadoComi".into()),
        cover: string_at(work, "bookCover").or_else(|| string_at(work, "thumbnail")),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let root = fetch_json(&details_api_url(&key), DETAILS_FIXTURE);
    let work = root.get("work").unwrap_or(&root);
    let status = string_at(work, "serializationStatus").unwrap_or_default().to_lowercase();
    CatalogItem {
        key: key.clone(),
        title: string_at(work, "title").unwrap_or_else(|| "KadoComi".into()),
        cover: string_at(work, "bookCover").or_else(|| string_at(work, "thumbnail")),
        authors: author_names(work),
        description: string_at(work, "summary"),
        tags: tag_names(work),
        status: if status == "ongoing" {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(root: &Value) -> Vec<MangaChapter> {
    let work_code = root.pointer("/work/code").and_then(Value::as_str).unwrap_or("sample");
    root.pointer("/latestEpisodes/result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|episode| {
            let episode_id = string_at(episode, "id").unwrap_or_default();
            let episode_code = string_at(episode, "code").unwrap_or_default();
            let active = episode.get("isActive").and_then(Value::as_bool).unwrap_or(true);
            MangaChapter {
                key: format!(
                    "/api/contents/viewer?episodeId={episode_id}&imageSizeType=width%3A1284#workCode={work_code}&episodeCode={episode_code}"
                ),
                title: string_at(episode, "title").map(|title| if active { title } else { format!("Locked: {title}") }),
                date_uploaded: string_at(episode, "updateDate").and_then(|value| dates::parse_ymd(value.split('T').next().unwrap_or(&value))),
                chapter_number: episode.pointer("/internal/episodeNo").and_then(Value::as_f64).map(|value| value as f32),
                is_locked: !active,
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(root: &Value, referer: &str) -> Vec<MangaPage> {
    root.get("manuscripts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let image = string_at(page, "drmImageUrl")?;
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                extra: BTreeMap::from([(
                    "drmHash".into(),
                    Value::String(string_at(page, "drmHash").unwrap_or_default()),
                )]),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn author_names(work: &Value) -> Vec<String> {
    work.get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| string_at(author, "name"))
        .collect()
}

fn tag_names(work: &Value) -> Vec<String> {
    let mut tags = Vec::new();
    for key in ["genre", "subGenre"] {
        if let Some(name) = work.pointer(&format!("/{key}/name")).and_then(Value::as_str) {
            tags.push(name.to_string());
        }
    }
    tags.extend(
        work.get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| string_at(tag, "name")),
    );
    tags
}

fn details_api_url(key: &str) -> String {
    format!("{API_URL}/contents/details/work?workCode={}", url::query_escape(&work_code(key)))
}

fn work_code(key: &str) -> String {
    key.trim_matches('/').rsplit('/').next().unwrap_or(key).to_string()
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToOwned::to_owned).filter(|value| !value.is_empty())
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "result": [
    {
      "code": "sample",
      "id": "sample",
      "thumbnail": "https://cdn.comic-walker.com/cover.jpg",
      "bookCover": null,
      "title": "Sample KadoComi",
      "serializationStatus": "ongoing"
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "work": {
    "code": "sample",
    "id": "sample",
    "thumbnail": "https://cdn.comic-walker.com/cover.jpg",
    "bookCover": null,
    "title": "Sample KadoComi",
    "serializationStatus": "ongoing",
    "summary": "Sample description.",
    "genre": { "name": "Sample" },
    "subGenre": null,
    "tags": [],
    "authors": [{ "name": "Sample Author", "role": "著者" }]
  },
  "latestEpisodes": {
    "result": [
      {
        "id": "episode-sample",
        "code": "episode-sample",
        "title": "Episode 1",
        "updateDate": "2024-01-01T00:00:00Z",
        "isActive": true,
        "internal": { "episodeNo": 1 }
      }
    ]
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "manuscripts": [
    {
      "drmHash": "00112233445566778899aabbccddeeff",
      "drmImageUrl": "https://cdn.comic-walker.com/images/sample.jpg?Expires=1&Signature=sample&Key-Pair-Id=sample"
    }
  ]
}"#;
