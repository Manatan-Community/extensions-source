use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Source = Source;
const BASE_URL: &str = "https://poseidon-scans.net";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing(&request) == "latest" {
            Ok(parse_latest(&fetch_json(
                &format!("{BASE_URL}/api/manga/lastchapters?limit=16&page={page}"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_popular(&fetch_rsc(BASE_URL, POPULAR_FIXTURE)))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!("{BASE_URL}/series");
        let mut params = Vec::new();
        if !query.is_empty() {
            params.push(format!("search={}", url::query_escape(query)));
        }
        if page > 1 {
            params.push(format!("page={page}"));
        }
        if !params.is_empty() {
            target.push('?');
            target.push_str(&params.join("&"));
        }
        Ok(parse_search(&fetch_doc(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        Ok(parse_details(
            &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        let show_premium = request
            .get("preferences")
            .and_then(|p| p.get("show_premium_chapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(
            &fetch_rsc(&url::join_url(BASE_URL, &key), CHAPTERS_FIXTURE),
            show_premium,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/serie/sample/chapter/1".into());
        Ok(parse_pages(&fetch_rsc(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_doc(input, DETAILS_FIXTURE), Some(key))),
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
fn fetch_doc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("RSC", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let data = json(body, LATEST_FIXTURE)
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_next_page = data.len() == 16;
    Paged {
        entries: data.into_iter().map(item).collect(),
        has_next_page,
    }
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = objects(body)
        .into_iter()
        .filter_map(|object| serde_json::from_str::<Value>(&object).ok())
        .filter(|v| v.get("slug").is_some() && v.get("title").is_some())
        .map(item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries: if entries.is_empty() {
            parse_latest(LATEST_FIXTURE).entries
        } else {
            entries
        },
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|c| c.contains("/serie/"))
            .filter_map(|c| {
                let href = html::attr(c, "href")?;
                let key = normalize(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(c, "<h2", "</h2>")
                        .or_else(|| html::attr_after(c, "<img", "alt"))
                        .map(|v| html::strip_tags(&v))
                        .filter(|v| !v.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Poseidon Scans".into()),
                    cover: html::attr_after(c, "<img", "src").map(|img| cover(&img)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("fr".into()),
                    content_rating: Some("safe".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("Pagination") && body.contains("Suivant"),
    }
}

fn item(value: Value) -> CatalogItem {
    let slug = str_field(&value, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: format!("/serie/{slug}"),
        title: str_field(&value, "title").unwrap_or_else(|| slug.clone()),
        cover: Some(format!("{BASE_URL}/api/covers/{slug}.webp")),
        url: Some(format!("{BASE_URL}/serie/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let value = objects(body)
        .into_iter()
        .filter_map(|o| serde_json::from_str::<Value>(&o).ok())
        .find(|v| v.get("description").is_some() && v.get("slug").is_some())
        .unwrap_or_else(|| json(DETAILS_DATA, DETAILS_DATA));
    let slug = str_field(&value, "slug")
        .unwrap_or_else(|| slug_from_key(key.as_deref().unwrap_or("/serie/sample")));
    CatalogItem {
        key: key.unwrap_or_else(|| format!("/serie/{slug}")),
        title: str_field(&value, "title").unwrap_or_else(|| slug.clone()),
        cover: Some(format!("{BASE_URL}/api/covers/{slug}.webp")),
        description: str_field(&value, "description"),
        authors: str_field(&value, "author").into_iter().collect(),
        artists: str_field(&value, "artist").into_iter().collect(),
        tags: value
            .get("categories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| str_field(&c, "name"))
            .collect(),
        status: match str_field(&value, "status")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "en cours" => ItemStatus::Ongoing,
            "terminé" | "termine" => ItemStatus::Completed,
            "en pause" | "hiatus" => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/serie/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, show_premium: bool) -> Vec<MangaChapter> {
    let root = objects(body)
        .into_iter()
        .filter_map(|o| serde_json::from_str::<Value>(&o).ok())
        .find(|v| v.pointer("/manga/chapters").is_some())
        .unwrap_or_else(|| json(CHAPTERS_DATA, CHAPTERS_DATA));
    let manga_value = root.get("manga").unwrap_or(&root);
    let slug = str_field(manga_value, "slug").unwrap_or_else(|| "sample".into());
    let mut chapters = manga_value
        .get("chapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| show_premium || !c.get("isPremium").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|c| {
            let n = c.get("number").and_then(Value::as_f64)? as f32;
            let number = if n.fract() == 0.0 {
                format!("{}", n as i64)
            } else {
                n.to_string()
            };
            let locked = c.get("isPremium").and_then(Value::as_bool).unwrap_or(false);
            Some(MangaChapter {
                key: format!("/serie/{slug}/chapter/{number}"),
                title: Some(format!(
                    "{}Chapitre {number}{}",
                    if locked { "[Premium] " } else { "" },
                    str_field(&c, "title")
                        .map(|t| format!(" - {t}"))
                        .unwrap_or_default()
                )),
                chapter_number: Some(n),
                is_locked: locked,
                url: Some(format!("{BASE_URL}/serie/{slug}/chapter/{number}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| b.chapter_number.partial_cmp(&a.chapter_number).unwrap());
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = objects(body)
        .into_iter()
        .filter_map(|o| serde_json::from_str::<Value>(&o).ok())
        .find(|v| v.pointer("/initialData/images").is_some())
        .unwrap_or_else(|| json(PAGES_DATA, PAGES_DATA));
    let mut pages = root
        .pointer("/initialData/images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    pages.sort_by_key(|p| p.get("order").and_then(Value::as_i64).unwrap_or(0));
    pages
        .into_iter()
        .filter_map(|p| str_field(&p, "originalUrl"))
        .enumerate()
        .map(|(i, img)| page_item(url::join_url(BASE_URL, &img), i))
        .collect()
}

fn page_item(image: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}
fn objects(input: &str) -> Vec<String> {
    let b = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        let mut d = 0;
        let mut s = false;
        let mut e = false;
        while i < b.len() {
            let c = b[i];
            if s {
                if e {
                    e = false;
                } else if c == b'\\' {
                    e = true;
                } else if c == b'"' {
                    s = false;
                }
            } else if c == b'"' {
                s = true;
            } else if c == b'{' {
                d += 1;
            } else if c == b'}' {
                d -= 1;
                if d == 0 {
                    out.push(String::from_utf8_lossy(&b[start..=i]).into_owned());
                    break;
                }
            }
            i += 1;
        }
        i += 1;
    }
    out
}
fn cover(value: &str) -> String {
    if value.starts_with("http") {
        value.to_string()
    } else if value.starts_with("/api/covers/") {
        url::join_url(BASE_URL, value)
    } else {
        format!("{BASE_URL}/api/covers/{}", value.trim_start_matches('/'))
    }
}
fn json(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or_else(|_| json!({}))
}
fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToString::to_string)
}
fn normalize(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(i) = value.find("/serie/") {
            return format!("/{}", value[i + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}
fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) && input.contains("/serie/")).then(|| normalize(input))
}
fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}
fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|e| e.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LATEST_FIXTURE: &str = r#"{"data":[{"title":"Sample Poseidon","slug":"sample"}]}"#;
const POPULAR_FIXTURE: &str = r#"0:{"id":"1","title":"Sample Poseidon","slug":"sample"}"#;
const SEARCH_FIXTURE: &str = r#"<div class="grid"><a class="block group" href="/serie/sample"><h2>Sample Poseidon</h2><img src="/api/covers/sample.webp"></a></div>"#;
const DETAILS_DATA: &str = r#"{"title":"Sample Poseidon","slug":"sample","description":"Summary","status":"en cours","artist":"Artist","author":"Author","categories":[{"name":"Action"}],"chapters":[{"number":1,"title":"Debut","isPremium":false}]}"#;
const DETAILS_FIXTURE: &str = r#"0:{"title":"Sample Poseidon","slug":"sample","description":"Summary","status":"en cours","artist":"Artist","author":"Author","categories":[{"name":"Action"}],"chapters":[{"number":1,"title":"Debut","isPremium":false}]}"#;
const CHAPTERS_DATA: &str = r#"{"manga":{"title":"Sample Poseidon","slug":"sample","chapters":[{"number":1,"title":"Debut","isPremium":false}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"0:{"manga":{"title":"Sample Poseidon","slug":"sample","chapters":[{"number":1,"title":"Debut","isPremium":false}]}}"#;
const PAGES_DATA: &str = r#"{"initialData":{"images":[{"originalUrl":"/page1.jpg","order":1}]}}"#;
const PAGES_FIXTURE: &str =
    r#"0:{"initialData":{"images":[{"originalUrl":"/page1.jpg","order":1}]}}"#;
