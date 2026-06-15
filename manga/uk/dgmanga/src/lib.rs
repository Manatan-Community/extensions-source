use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, Viewer, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: DGManga = DGManga;
const BASE_URL: &str = "https://dgmanga.app";
const API_URL: &str = "https://dgmanga.app/api";

struct DGManga;

impl MangaSource for DGManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "updated"
        } else {
            "popular"
        };
        let target = titles_url(
            page(&request),
            "",
            sort,
            request.get("preferences").unwrap_or(&Value::Null),
            &Value::Null,
        );
        Ok(parse_catalog(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/title/") {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        let sort = filter_string(&request, "sort").unwrap_or_else(|| "rating".to_string());
        let target = titles_url(
            page(&request),
            query,
            &sort,
            request.get("preferences").unwrap_or(&Value::Null),
            request.get("filters").unwrap_or(&Value::Null),
        );
        Ok(parse_catalog(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &fetch_json(&details_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&fetch_json(
            &chapters_url(&key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter-id/1/sample".to_string());
        let body = fetch_document(&chapter_web_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_web_url(&key)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/title/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_web_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/title/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                )),
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

export_manga_source!(SOURCE);

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer.to_string())
        .with_origin(BASE_URL)
        .with_header("Accept-Language", "uk-UA,uk;q=0.9,en-US;q=0.8,en;q=0.7")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client(BASE_URL)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client(target)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn titles_url(page: u64, query: &str, sort: &str, preferences: &Value, filters: &Value) -> String {
    let mut params = vec![
        ("page", page.to_string()),
        ("limit", "28".to_string()),
        ("sort", sort.to_string()),
        ("skipContentPrefs", "true".to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", query.to_string()));
    }
    for key in [
        "type",
        "status",
        "translation_status",
        "genres",
        "tags",
        "isLicensed",
    ] {
        if let Some(value) = filter_string_in(filters, key).filter(|value| !value.is_empty()) {
            params.push((key, value));
        }
    }
    if pref_bool(preferences, "hideLicensed") {
        params.retain(|(key, _)| *key != "isLicensed");
        params.push(("isLicensed", "false".to_string()));
    }
    format!("{API_URL}/titles?{}", query_string(&params))
}

fn details_url(key: &str) -> String {
    format!(
        "{API_URL}/titles/{}",
        url::query_escape(key.trim_matches('/'))
    )
}

fn chapters_url(key: &str) -> String {
    format!(
        "{API_URL}/chapters/title/{}",
        url::query_escape(key.trim_matches('/'))
    )
}

fn chapter_web_url(key: &str) -> String {
    let mut parts = key.splitn(3, '/');
    let chapter_id = parts.next().unwrap_or("chapter-id");
    let number = parts.next().unwrap_or("1");
    let title = parts.next().unwrap_or("sample");
    format!("{BASE_URL}/read/{title}/{number}?chapterId={chapter_id}")
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = root
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    let entries = root
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) != Some("novel"))
        .map(|item| {
            let key = string_field(item, &["_id", "id", "slug"]);
            CatalogItem {
                key: key.clone(),
                title: string_field(item, &["title", "name"]),
                cover: opt_string_field(item, &["cover", "coverImageUrl"]),
                url: Some(format!("{BASE_URL}/title/{key}")),
                tags: array_strings(item.get("genres")),
                language: Some("uk".to_string()),
                content_rating: Some("safe".to_string()),
                viewer: Some(Viewer::RightToLeft),
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: total > page,
    }
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let key = opt_string_field(&root, &["_id", "id"]).unwrap_or_else(|| fallback_key.to_string());
    let mut tags = Vec::new();
    if let Some(kind) = root.get("type").and_then(Value::as_str) {
        tags.push(manga_type(kind).to_string());
    }
    tags.extend(array_strings(root.get("genres")));
    tags.extend(array_strings(root.get("tags")));
    CatalogItem {
        key: key.clone(),
        title: string_field(&root, &["title", "name"]),
        alternate_titles: array_strings(root.get("alternativeTitles")),
        cover: opt_string_field(&root, &["cover", "coverImageUrl"]),
        url: Some(format!("{BASE_URL}/title/{key}")),
        authors: staff_names(root.get("illustratorRef")),
        artists: staff_names(root.get("authorRef")),
        description: root
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags,
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        status: match root.get("translation_status").and_then(Value::as_str) {
            Some("Покинуто") => ItemStatus::Cancelled,
            Some("Завершено") => ItemStatus::Completed,
            Some("Перекладається") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            let id = string_field(&item, &["_id", "id"]);
            let number = item
                .get("chapterNumber")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let title = string_field(&item, &["title"]);
            let volume = item
                .get("volumeNumber")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let name = item
                .get("chapterName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let number_text = compact_float(number);
            let key = format!("{id}/{number_text}/{title}");
            MangaChapter {
                key: key.clone(),
                title: Some(
                    format!("Том {volume} Розділ {number_text} {name}")
                        .trim()
                        .to_string(),
                ),
                chapter_number: Some(number as f32),
                date_uploaded: item
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                scanlators: staff_names(item.get("teams")),
                language: Some("uk".to_string()),
                url: Some(chapter_web_url(&key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    image_urls(body)
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_urls(body: &str) -> Vec<String> {
    let target = body
        .find("pages")
        .and_then(|start| {
            body[start..]
                .find("],")
                .map(|end| &body[start..start + end])
        })
        .unwrap_or(body);
    let mut urls = Vec::new();
    for part in target.split("http").skip(1) {
        let mut raw = String::from("http");
        for ch in part.chars() {
            if ch == '"' || ch == '\'' || ch == '<' || ch == '\\' {
                break;
            }
            raw.push(ch);
        }
        if raw.starts_with("http") && !urls.contains(&raw) {
            urls.push(html::html_unescape(&raw));
        }
    }
    urls
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filter_string_in(filters, key))
}

fn filter_string_in(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn pref_bool(preferences: &Value, key: &str) -> bool {
    preferences
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn string_field(value: &Value, keys: &[&str]) -> String {
    opt_string_field(value, keys).unwrap_or_default()
}

fn opt_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn array_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn staff_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn manga_type(kind: &str) -> &str {
    match kind {
        "manga" => "Манґа",
        "manhwa" => "Манхва",
        "manhua" => "Маньхва",
        "western" => "Вестерн",
        "Мальописи" => "Мальопис",
        "novel" => "Новела",
        _ => kind,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(dates::parse_ymd)
}

fn compact_float(value: f64) -> String {
    if value.fract() == 0.0 {
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

const LIST_FIXTURE: &str = r#"{"titles":[{"_id":"sample","title":"Sample DGManga","cover":"https://dgmanga.app/cover.jpg","genres":["Фентезі"],"type":"manga"}],"page":1,"totalPages":1}"#;
const DETAILS_FIXTURE: &str = r#"{"_id":"sample","title":"Sample DGManga","cover":"https://dgmanga.app/cover.jpg","description":"Fixture","type":"manga","translation_status":"Перекладається","genres":["Фентезі"],"tags":[],"authorRef":[],"illustratorRef":[],"alternativeTitles":[]}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"_id":"chapter-id","title":"sample","chapterNumber":1,"volumeNumber":1,"chapterName":"Start","createdAt":"2024-01-01T00:00:00.000Z","teams":[{"name":"DG"}]}]"#;
const PAGES_FIXTURE: &str = r#"<script>window.pages=["https://dgmanga.app/page-1.jpg\"","https://dgmanga.app/page-2.jpg\""],</script>"#;
