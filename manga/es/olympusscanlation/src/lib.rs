use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: OlympusScanlation = OlympusScanlation;
const DEFAULT_BASE_URL: &str = "https://olympusbiblioteca.com";
const DOMAIN_DISCOVERY_URL: &str = "https://olympus.pages.dev";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";
const PAGE_SIZE: usize = 20;

struct OlympusScanlation;

impl MangaSource for OlympusScanlation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(RANKING_FIXTURE, 1, base_url(&request)));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if listing_id(&request) == "latest" {
            format!("{base}/api/new-chapters?page={page}")
        } else {
            format!("{base}/api/rankings?page={page}&period=total_ranking")
        };
        Ok(parse_listing(
            &fetch_json(&base, &path, RANKING_FIXTURE),
            page,
            base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = olympus_slug(query) {
            return Ok(Paged {
                entries: vec![details_from_slug(&base, &slug, None)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_json(
            &base,
            &format!("{base}/api/series/list"),
            SERIES_LIST_FIXTURE,
        );
        let needle = query.to_ascii_lowercase();
        let matches = json_or_fixture(&body, SERIES_LIST_FIXTURE)
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| string_value(item, "type").as_deref() == Some("comic"))
            .filter(|item| {
                needle.is_empty()
                    || string_value(item, "name")
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&needle)
            })
            .cloned()
            .collect::<Vec<_>>();
        let start = (page.saturating_sub(1) as usize) * PAGE_SIZE;
        let entries = matches
            .iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(|item| catalog_from_json(item, &base, false))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: start + PAGE_SIZE < matches.len(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic-sample#1".to_string());
        let (slug, id) = split_series_key(&key);
        Ok(details_from_slug(&base, &slug, id))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/series/comic-sample#1".to_string());
        let (slug, id) = split_series_key(&key);
        Ok(fetch_chapters(&base, &slug, id.unwrap_or("1")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/capitulo/1/comic-sample#1".to_string());
        let (slug, chapter_id) = split_chapter_key(&key);
        let body = fetch_json(
            &base,
            &format!("{base}/api/capitulo/comic-{slug}/{chapter_id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &base))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| {
            let (slug, _) = split_series_key(&key);
            format!("{base}/series/comic-{slug}")
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, chapter_id) = split_chapter_key(&key);
            format!("{base}/capitulo/{chapter_id}/comic-{slug}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = olympus_slug(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(&base, &slug, None)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base.trim_end_matches('/')))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_json(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    let prefs = request.get("preferences").and_then(Value::as_object);
    let configured = prefs
        .and_then(|prefs| {
            prefs
                .get("overrideBaseUrl")
                .or_else(|| prefs.get("pref_overrideBaseUrl"))
                .and_then(Value::as_str)
        })
        .filter(|value| value.starts_with("http"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string();
    let fetch_domain = prefs
        .and_then(|prefs| {
            prefs
                .get("fetchDomain")
                .or_else(|| prefs.get("pref_fetchDomain"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(true);
    if !fetch_domain {
        return configured;
    }
    let body = client(&configured)
        .get(DOMAIN_DISCOVERY_URL)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    html::attr_after(&body, "property=\"og:url\"", "content")
        .or_else(|| html::attr_after(&body, "property='og:url'", "content"))
        .and_then(|value| {
            if value.starts_with("http") {
                Some(value.trim_end_matches('/').to_string())
            } else {
                None
            }
        })
        .unwrap_or(configured)
}

fn api_dashboard(base: &str) -> String {
    base.replacen("https://", "https://dashboard.", 1)
}

fn parse_listing(body: &str, page: u64, base: String) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, RANKING_FIXTURE);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| string_value(item, "type").as_deref() == Some("comic"))
        .map(|item| catalog_from_json(item, &base, false))
        .collect();
    let last_page = root
        .get("last_page")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    let current_page = root
        .get("current_page")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries,
        has_next_page: current_page < last_page,
    }
}

fn details_from_slug(base: &str, slug: &str, fallback_id: Option<&str>) -> CatalogItem {
    let body = fetch_json(
        base,
        &format!("{base}/api/series/{slug}?type=comic"),
        DETAILS_FIXTURE,
    );
    let root = json_or_fixture(&body, DETAILS_FIXTURE);
    let item = root.get("data").unwrap_or(&Value::Null);
    let mut out = catalog_from_json(item, base, true);
    if out.key.ends_with("#0") {
        out.key = format!(
            "/series/comic-{}#{}",
            normalize_slug(slug),
            fallback_id.unwrap_or("0")
        );
    }
    out
}

fn catalog_from_json(item: &Value, base: &str, initialized: bool) -> CatalogItem {
    let id = item.get("id").and_then(Value::as_i64).unwrap_or(0);
    let slug = string_value(item, "slug").unwrap_or_else(|| "sample".to_string());
    CatalogItem {
        key: format!("/series/comic-{}#{id}", normalize_slug(&slug)),
        title: string_value(item, "name").unwrap_or_else(|| "Olympus Scanlation".to_string()),
        cover: string_value(item, "cover"),
        description: string_value(item, "summary").map(|value| html::strip_tags(&value)),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| string_value(genre, "name"))
            .collect(),
        status: status_from_id(
            item.get("status")
                .and_then(|status| status.get("id"))
                .and_then(Value::as_i64),
        ),
        url: Some(format!("{base}/series/comic-{}", normalize_slug(&slug))),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(base: &str, slug: &str, manga_id: &str) -> Vec<MangaChapter> {
    let dashboard = api_dashboard(base);
    let mut page = 1;
    let mut chapters = Vec::new();
    loop {
        let body = fetch_json(
            base,
            &format!(
                "{dashboard}/api/series/{slug}/chapters?page={page}&direction=desc&type=comic"
            ),
            CHAPTERS_FIXTURE,
        );
        let root = json_or_fixture(&body, CHAPTERS_FIXTURE);
        let data = root
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        chapters.extend(data.iter().filter_map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64)?;
            let name = string_value(chapter, "name").unwrap_or_else(|| id.to_string());
            Some(MangaChapter {
                key: format!("/capitulo/{id}/comic-{slug}#{manga_id}"),
                title: Some(format!("Capitulo {name}")),
                date_uploaded: string_value(chapter, "published_at")
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(format!("{base}/capitulo/{id}/comic-{slug}")),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        }));
        let total = root
            .get("meta")
            .and_then(|meta| meta.get("total"))
            .and_then(Value::as_u64)
            .unwrap_or(chapters.len() as u64);
        if chapters.len() as u64 >= total || page >= 20 {
            break;
        }
        page += 1;
    }
    chapters
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    root.get("chapter")
        .and_then(|chapter| chapter.get("pages"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(base, image),
                context: None,
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn olympus_slug(input: &str) -> Option<String> {
    input
        .split("/series/comic-")
        .nth(1)
        .map(|value| normalize_slug(value.split(['?', '#']).next().unwrap_or(value)))
}

fn split_series_key(key: &str) -> (String, Option<&str>) {
    let id = key.split('#').nth(1);
    let slug = key
        .split('#')
        .next()
        .unwrap_or(key)
        .trim_matches('/')
        .trim_start_matches("series/")
        .trim_start_matches("comic-");
    (normalize_slug(slug), id)
}

fn split_chapter_key(key: &str) -> (String, String) {
    let clean = key.split('#').next().unwrap_or(key).trim_matches('/');
    let mut parts = clean.split('/');
    let chapter_id = parts.nth(1).unwrap_or("1").to_string();
    let slug = parts
        .next()
        .unwrap_or("comic-sample")
        .trim_start_matches("comic-")
        .to_string();
    (normalize_slug(&slug), chapter_id)
}

fn normalize_slug(input: &str) -> String {
    input
        .trim()
        .trim_matches('/')
        .trim_start_matches("comic-")
        .to_string()
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn string_value(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn status_from_id(status: Option<i64>) -> ItemStatus {
    match status {
        Some(1) => ItemStatus::Ongoing,
        Some(3) => ItemStatus::Hiatus,
        Some(4) => ItemStatus::Completed,
        Some(5) => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.get(0..10)?;
    manatan_shared::dates::parse_ymd(date)
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{
  "data": [{ "id": 1, "name": "Sample Manga", "slug": "sample", "cover": "/cover.jpg", "type": "comic" }],
  "current_page": 1,
  "last_page": 1
}"#;
const SERIES_LIST_FIXTURE: &str = r#"{
  "data": [{ "id": 1, "name": "Sample Manga", "slug": "sample", "cover": "/cover.jpg", "type": "comic" }]
}"#;
const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "id": 1,
    "name": "Sample Manga",
    "slug": "sample",
    "cover": "/cover.jpg",
    "summary": "<p>Sample description</p>",
    "status": { "id": 1 },
    "genres": [{ "name": "Accion" }]
  }
}"#;
const CHAPTERS_FIXTURE: &str = r#"{
  "data": [{ "id": 1, "name": "1", "published_at": "2024-01-01T00:00:00.000000Z" }],
  "meta": { "total": 1 }
}"#;
const PAGES_FIXTURE: &str = r#"{ "chapter": { "pages": ["/page1.jpg"] } }"#;
