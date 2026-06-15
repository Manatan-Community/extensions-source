use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: KiraKira = KiraKira;
const BASE_URL: &str = "https://truyenkira.com";
const API_URL: &str = "https://api.truyenkira.com";

struct KiraKira;

impl MangaSource for KiraKira {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            format!("{API_URL}/top?status=all&page={page}")
        } else {
            format!("{API_URL}/recent-update-comics?page={page}")
        };
        Ok(parse_list(&fetch_json(&target, LIST_FIXTURE)))
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
        let target = if !query.is_empty() {
            format!(
                "{API_URL}/search?q={}&page={page}",
                url::query_escape(query)
            )
        } else if let Some(genre) = request
            .get("filters")
            .and_then(|f| f.get("genre"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            format!("{API_URL}/genres/{genre}?type={genre}&page={page}")
        } else {
            format!("{API_URL}/recent-update-comics?page={page}")
        };
        Ok(parse_list(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let auto_unlock = preference_bool(&request, "autoUnlockChapters");
        let slug = comic_slug(&key).unwrap_or("sample");
        let body = fetch_json(&format!("{API_URL}/comics/{slug}"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, slug, auto_unlock))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapters/sample/1".into());
        if key.contains("is_locked=1") && !preference_bool(&request, "autoUnlockChapters") {
            return Ok(vec![manga::text_page(
                "Chapter is locked. Enable auto unlock or log in with WebView and a matching account.",
            )]);
        }
        let (slug, chapter_id) = chapter_info(&key).unwrap_or(("sample", "1"));
        let body = fetch_json(
            &format!("{API_URL}/comics/{slug}/chapters/{chapter_id}"),
            PAGES_FIXTURE,
        );
        let pages = parse_pages(&body);
        if pages.is_empty() {
            return Ok(vec![manga::text_page(
                "No images were returned for this chapter. The upstream auto-unlock image probing fallback is not available in this Manatan port.",
            )]);
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
                item: key.starts_with("/comics/").then(|| details_by_key(&key)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = value
        .get("comics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|comic| {
            let slug = comic.get("id").and_then(Value::as_str)?;
            let title = comic
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(slug)
                .to_string();
            Some(CatalogItem {
                key: format!("/comics/{slug}"),
                title,
                cover: comic
                    .get("thumbnail")
                    .or_else(|| comic.get("banner_image_url"))
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string),
                url: Some(format!("{BASE_URL}/comics/{slug}")),
                language: Some("vi".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let current = value
        .get("current_page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = value
        .get("total_pages")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < total,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let slug = comic_slug(key).unwrap_or("sample");
    parse_details(
        &fetch_json(&format!("{API_URL}/comics/{slug}"), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    CatalogItem {
        key: key.into(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_else(|| comic_slug(key).unwrap_or("Manga"))
            .to_string(),
        cover: value
            .get("thumbnail")
            .or_else(|| value.get("banner_image_url"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        authors: vec!["Unknown".into()],
        tags: value
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        status: match value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "updating" | "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, slug: &str, auto_unlock: bool) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    value
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let id = chapter.get("id").map(value_string)?;
            let name = chapter
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Chapter")
                .to_string();
            let locked = chapter
                .get("coinPrice")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                > 0;
            let key = if locked && !auto_unlock {
                format!("/chapters/{slug}/{id}?is_locked=1")
            } else {
                format!("/chapters/{slug}/{id}")
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if locked && !auto_unlock {
                    format!("Locked {name}")
                } else {
                    name
                }),
                url: Some(absolute_url(&key)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    value
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| image.get("src").and_then(Value::as_str))
        .filter(|src| !src.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.into(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn value_string(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn comic_slug(key: &str) -> Option<&str> {
    key.split("/comics/")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|v| !v.is_empty())
}

fn chapter_info(key: &str) -> Option<(&str, &str)> {
    let rest = key
        .split("/chapters/")
        .nth(1)
        .or_else(|| key.split("/comics/").nth(1))?;
    let mut parts = rest.split(['/', '?', '#']).filter(|part| !part.is_empty());
    Some((parts.next()?, parts.next()?))
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else {
        format!("{BASE_URL}/{}", value.trim_start_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.starts_with("/comics/") || key.starts_with("/chapters/"))
}

const LIST_FIXTURE: &str = r#"{"comics":[{"id":"sample","title":"Sample","thumbnail":"https://truyenkira.com/cover.jpg"}],"current_page":1,"total_pages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":"Sample","thumbnail":"https://truyenkira.com/cover.jpg","description":"Summary","status":"ongoing","genres":[{"name":"Action"}],"chapters":[{"id":1,"name":"Chapter 1","coinPrice":0}]}"#;
const PAGES_FIXTURE: &str = r#"{"images":[{"page":1,"src":"https://truyenkira.com/manga/sample/chapter-1/page-1.jpg"}],"coinPrice":0,"isPurchased":true}"#;

export_manga_source!(SOURCE);
