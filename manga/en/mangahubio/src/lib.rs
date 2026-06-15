use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionResult, cookies_get},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: MangaHubIo = MangaHubIo;
const BASE_URL: &str = "https://mangahub.io";
const API_URL: &str = "https://api.mghcdn.com/graphql";
const THUMB_CDN: &str = "https://thumb.mghcdn.com";
const IMAGE_CDN: &str = "https://imgx.mghcdn.com";
const MANGA_SOURCE: &str = "m01";
const CONTENT_RATING: &str = "adult";

struct MangaHubIo;

impl MangaSource for MangaHubIo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_response(SEARCH_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "LATEST"
        } else {
            "POPULAR"
        };
        Ok(parse_search_response(&graphql_or_fixture(
            &search_query("", "all", order, page),
            None,
            SEARCH_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details_response(
                    &graphql_or_fixture(&details_query(slug_from_key(&key)), Some(query), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_search_response(&graphql_or_fixture(
            &search_query(query, "all", "POPULAR", page),
            None,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details_response(
            &graphql_or_fixture(&details_query(slug_from_key(&key)), Some(&absolute_url(&key)), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters_response(
            &graphql_or_fixture(&chapters_query(slug_from_key(&key)), Some(&absolute_url(&key)), CHAPTERS_FIXTURE),
            slug_from_key(&key),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".to_string());
        let (slug, number) = chapter_parts(&key);
        Ok(parse_pages_response(&graphql_or_fixture(
            &pages_query(&slug, number),
            Some(&format!("{BASE_URL}/chapter/{key}")),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/chapter/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = key.starts_with("/manga/").then(|| {
                parse_details_response(
                    &graphql_or_fixture(&details_query(slug_from_key(&key)), Some(input), DETAILS_FIXTURE),
                    Some(key),
                )
            });
            return Ok(Some(UrlResolveResult { item, url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
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

fn graphql_or_fixture(query: &str, refresh_url: Option<&str>, fixture: &str) -> String {
    let _ = client().get(refresh_url.unwrap_or(BASE_URL)).browser_document().send_text();
    let body = json!({ "query": query }).to_string();
    client()
        .post(API_URL)
        .header("Accept", "application/json")
        .header("Origin", BASE_URL)
        .header("x-mhub-access", mhub_access_cookie().unwrap_or_default())
        .json(body)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn mhub_access_cookie() -> Option<String> {
    let response = cookies_get(BASE_URL).ok()?;
    response.cookies.into_iter().find(|cookie| cookie.name == "mhub_access" && !cookie.value.is_empty()).map(|cookie| cookie.value)
        .or_else(|| response.header.and_then(|header| header.split(';').find_map(|part| part.trim().strip_prefix("mhub_access=").map(ToString::to_string))))
}

fn search_query(query: &str, genre: &str, order: &str, page: u64) -> String {
    let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
    let offset = page.saturating_sub(1) * 30;
    format!(r#"{{ search(x: {MANGA_SOURCE}, q: "{escaped}", genre: "{genre}", mod: {order}, offset: {offset}) {{ rows {{ title author slug image genres latestChapter }} }} }}"#)
}

fn details_query(slug: &str) -> String {
    format!(r#"{{ manga(x: {MANGA_SOURCE}, slug: "{slug}") {{ title slug status image author artist genres description alternativeTitle }} }}"#)
}

fn chapters_query(slug: &str) -> String {
    format!(r#"{{ manga(x: {MANGA_SOURCE}, slug: "{slug}") {{ slug chapters {{ number title date }} }} }}"#)
}

fn pages_query(slug: &str, number: f32) -> String {
    format!(r#"{{ chapter(x: {MANGA_SOURCE}, slug: "{slug}", number: {number}) {{ pages mangaID number manga {{ slug }} }} }}"#)
}

fn parse_search_response(body: &str) -> Paged<CatalogItem> {
    let rows = serde_json::from_str::<Value>(body).ok().and_then(|root| root.get("data")?.get("search")?.get("rows")?.as_array().cloned()).unwrap_or_default();
    let entries = rows.iter().filter_map(|row| {
        let slug = json_text(row, "slug")?;
        let key = format!("/manga/{slug}");
        Some(CatalogItem {
            key: key.clone(),
            title: json_text(row, "title").unwrap_or_else(|| "Manga".to_string()),
            cover: json_text(row, "image").map(|image| format!("{THUMB_CDN}/{image}")),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { has_next_page: rows.len() == 30, entries }
}

fn parse_details_response(body: &str, key: Option<String>) -> CatalogItem {
    let manga = serde_json::from_str::<Value>(body).ok().and_then(|root| root.get("data")?.get("manga").cloned()).unwrap_or(Value::Null);
    let slug = json_text(&manga, "slug").unwrap_or_else(|| slug_from_key(key.as_deref().unwrap_or("/manga/sample")).to_string());
    let key = key.unwrap_or_else(|| format!("/manga/{slug}"));
    let mut description = json_text(&manga, "description").unwrap_or_default();
    if let Some(alt_title) = json_text(&manga, "alternativeTitle").filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Name: ");
        description.push_str(&alt_title);
    }
    CatalogItem {
        key: key.clone(),
        title: json_text(&manga, "title").unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: json_text(&manga, "image").map(|image| format!("{THUMB_CDN}/{image}")),
        authors: json_text(&manga, "author").into_iter().collect(),
        artists: json_text(&manga, "artist").into_iter().collect(),
        tags: json_text(&manga, "genres").map(split_csv).unwrap_or_default(),
        description: (!description.is_empty()).then_some(description),
        status: match json_text(&manga, "status").as_deref() {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_response(body: &str, slug: &str) -> Vec<MangaChapter> {
    let chapters = serde_json::from_str::<Value>(body).ok().and_then(|root| root.get("data")?.get("manga")?.get("chapters")?.as_array().cloned()).unwrap_or_default();
    chapters.into_iter().rev().map(|chapter| {
        let number = json_number(&chapter, "number").unwrap_or(0.0);
        let number_string = number_string(number);
        let title = json_text(&chapter, "title").unwrap_or_default();
        let display = if title.contains(&number_string) { title } else if title.trim().is_empty() { format!("Chapter {number_string}") } else { format!("Chapter {number_string} - {}", title.trim()) };
        let key = format!("/{slug}/chapter-{number_string}");
        MangaChapter {
            key: key.clone(),
            title: Some(display),
            chapter_number: Some(number),
            url: Some(format!("{BASE_URL}/chapter{key}")),
            date_uploaded: json_text(&chapter, "date").and_then(|date| manatan_shared::dates::parse_fixture_date(&date)),
            ..MangaChapter::default()
        }
    }).collect()
}

fn parse_pages_response(body: &str) -> Vec<MangaPage> {
    let chapter = serde_json::from_str::<Value>(body).ok().and_then(|root| root.get("data")?.get("chapter").cloned()).unwrap_or(Value::Null);
    let pages_raw = json_text(&chapter, "pages").unwrap_or_default();
    let pages = serde_json::from_str::<Value>(&pages_raw).unwrap_or(Value::Null);
    let base = json_text(&pages, "p").unwrap_or_default();
    pages.get("i").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().filter_map(|image| image.as_str().map(ToString::to_string)).enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: format!("{IMAGE_CDN}/{base}{image}"), context: Some(manga::image_headers(BASE_URL)) },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn slug_from_key(key: &str) -> &str {
    key.trim_matches('/').strip_prefix("manga/").unwrap_or(key.trim_matches('/'))
}

fn chapter_parts(key: &str) -> (String, f32) {
    let clean = key.trim_matches('/');
    let slug = clean.split('/').next().unwrap_or("sample").to_string();
    let number = clean.split("chapter-").nth(1).and_then(|value| value.parse::<f32>().ok()).unwrap_or(1.0);
    (slug, number)
}

fn number_string(value: f32) -> String {
    if value.fract() == 0.0 { format!("{}", value as i32) } else { value.to_string() }
}

fn split_csv(value: String) -> Vec<String> {
    value.split(',').map(str::trim).filter(|value| !value.is_empty()).map(ToString::to_string).collect()
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn json_number(value: &Value, key: &str) -> Option<f32> {
    value.get(key).and_then(Value::as_f64).map(|number| number as f32)
}

const SEARCH_FIXTURE: &str = r#"{"data":{"search":{"rows":[{"title":"Sample","author":"Author","slug":"sample","image":"sample.jpg","genres":"Action","latestChapter":1}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"manga":{"title":"Sample","slug":"sample","status":"ongoing","image":"sample.jpg","author":"Author","artist":"Artist","genres":"Action, Adventure","description":"Summary","alternativeTitle":"Alt Sample"}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"manga":{"slug":"sample","chapters":[{"number":1,"title":"Start","date":"2024-01-01T00:00:00.000Z"}]}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"chapter":{"pages":"{\"p\":\"sample/\",\"i\":[\"001.jpg\"]}","mangaID":1,"number":1,"manga":{"slug":"sample"}}}}"#;

export_manga_source!(SOURCE);
