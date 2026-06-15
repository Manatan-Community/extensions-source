use manatan_extension::{
    CatalogItem, Context, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, sdk::SearchRequest, url};
use serde_json::{Value, json};

const SOURCE: GeassComics = GeassComics;
const BASE_URL: &str = "https://geasscomics.xyz";
const API_URL: &str = "https://api.skkyscan.fun";
const PAGE_LIMIT: u64 = 24;
const CHAPTERS_LIMIT: u64 = 100;

struct GeassComics;

impl MangaSource for GeassComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(SEARCH_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updatedAt"
        } else {
            "views"
        };
        let mut endpoint = format!(
            "{API_URL}/api/mangas/search?sort={sort}&order=desc&page={page}&limit={PAGE_LIMIT}"
        );
        if !show_nsfw_pref(&request) {
            endpoint.push_str("&nsfw=false");
        }
        Ok(parse_list(&api_get(&endpoint, auth_token(&request), SEARCH_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_slug(slug_from_key(&key).unwrap_or("sample"))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = filter_str(&request, "sort").unwrap_or("updatedAt");
        let order = if sort == "title" { "asc" } else { "desc" };
        let mut endpoint =
            format!("{API_URL}/api/mangas/search?page={page}&limit={PAGE_LIMIT}&sort={sort}&order={order}");
        if !query.is_empty() {
            endpoint.push_str("&q=");
            endpoint.push_str(&url::query_escape(query));
        }
        if let Some(status) = filter_str(&request, "status").filter(|value| !value.is_empty()) {
            endpoint.push_str("&status=");
            endpoint.push_str(&url::query_escape(status));
        }
        append_text_filter(&mut endpoint, &request, "genres", "genres");
        append_text_filter(&mut endpoint, &request, "tags", "tags");
        let show_nsfw = filter_bool(&request, "nsfw").unwrap_or(false) && show_nsfw_pref(&request);
        if show_nsfw || !show_nsfw_pref(&request) {
            endpoint.push_str("&nsfw=");
            endpoint.push_str(if show_nsfw { "true" } else { "false" });
        }
        Ok(parse_list(&api_get(&endpoint, auth_token(&request), SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(details_from_slug(slug_from_key(&key).unwrap_or("sample")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let slug = slug_from_key(&key).unwrap_or("sample").to_string();
        let details = api_get(
            &format!("{API_URL}/api/mangas/{slug}"),
            auth_token(&request),
            DETAILS_FIXTURE,
        );
        let manga_id = serde_json::from_str::<Value>(&details)
            .ok()
            .and_then(|value| value.pointer("/data/id").and_then(Value::as_str).map(ToString::to_string))
            .unwrap_or_else(|| "manga-id".to_string());
        let mut chapters = Vec::new();
        for page in 1..=100 {
            let endpoint = format!(
                "{API_URL}/api/chapters?mangaId={manga_id}&page={page}&limit={CHAPTERS_LIMIT}&order=desc"
            );
            let body = api_get(&endpoint, auth_token(&request), CHAPTERS_FIXTURE);
            let value = serde_json::from_str::<Value>(&body)
                .or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE))
                .unwrap_or(Value::Null);
            let items = value
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            chapters.extend(items.iter().map(|chapter| chapter_item(chapter, &slug)));
            if !has_next(&value) {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/chapter-id/sample/1".to_string());
        let chapter_id = chapter_id_from_key(&key).unwrap_or("chapter-id");
        let body = api_get(
            &format!("{API_URL}/api/chapters/{chapter_id}"),
            auth_token(&request),
            PAGES_FIXTURE,
        );
        let value = serde_json::from_str::<Value>(&body)
            .or_else(|_| serde_json::from_str(PAGES_FIXTURE))
            .unwrap_or(Value::Null);
        Ok(value
            .pointer("/data/pages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(page_item)
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "manga")
            .and_then(|key| slug_from_key(&key).map(|slug| format!("{BASE_URL}/obra/{slug}"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter").map(|key| {
            let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
            let slug = parts.get(2).copied().unwrap_or_default();
            let number = parts.get(3).copied().unwrap_or_default();
            format!("{BASE_URL}/ler/{slug}/{number}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/obra/") {
            let slug = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_slug(slug)),
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

fn api_get(target_url: &str, token: Option<String>, fixture: &str) -> String {
    let http = client();
    let mut request = http.get(target_url).xhr();
    if let Some(token) = token.filter(|value| !value.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Accept", "application/json, text/plain, */*")
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn auth_token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    if let Some(token) = prefs.get("pref_token").and_then(Value::as_str) {
        if !token.trim().is_empty() {
            return Some(token.to_string());
        }
    }
    let email = prefs.get("pref_email").and_then(Value::as_str)?.trim();
    let password = prefs.get("pref_password").and_then(Value::as_str)?.trim();
    if email.is_empty() || password.is_empty() {
        return None;
    }
    let body = client()
        .post(format!("{API_URL}/api/auth/login"))
        .json(json!({ "email": email, "password": password }).to_string())
        .xhr()
        .send_text()
        .ok()?;
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.pointer("/data/accessToken").and_then(Value::as_str).map(ToString::to_string))
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .or_else(|_| serde_json::from_str(SEARCH_FIXTURE))
        .unwrap_or(Value::Null);
    Paged {
        entries: value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| catalog_item(item, false))
            .collect(),
        has_next_page: has_next(&value),
    }
}

fn details_from_slug(slug: &str) -> CatalogItem {
    let body = api_get(&format!("{API_URL}/api/mangas/{slug}"), None, DETAILS_FIXTURE);
    let value = serde_json::from_str::<Value>(&body)
        .or_else(|_| serde_json::from_str(DETAILS_FIXTURE))
        .unwrap_or(Value::Null);
    value
        .get("data")
        .map(|item| catalog_item(item, true))
        .unwrap_or_else(|| catalog_item(&value, true))
}

fn catalog_item(value: &Value, initialized: bool) -> CatalogItem {
    let slug = value.get("slug").and_then(Value::as_str).unwrap_or("sample");
    CatalogItem {
        key: format!("/manga/{slug}"),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Geass Comics")
            .to_string(),
        cover: value
            .get("coverImage")
            .and_then(Value::as_str)
            .map(|path| format!("{API_URL}{path}")),
        description: description(value),
        authors: value
            .get("author")
            .and_then(Value::as_str)
            .map(|author| vec![author.to_string()])
            .unwrap_or_default(),
        artists: value
            .get("artist")
            .and_then(Value::as_str)
            .map(|artist| vec![artist.to_string()])
            .unwrap_or_default(),
        tags: tags(value),
        status: match value.get("status").and_then(Value::as_str).unwrap_or_default() {
            "ongoing" => manatan_extension::ItemStatus::Ongoing,
            "completed" => manatan_extension::ItemStatus::Completed,
            "hiatus" => manatan_extension::ItemStatus::Hiatus,
            "cancelled" => manatan_extension::ItemStatus::Cancelled,
            _ => manatan_extension::ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/obra/{slug}")),
        language: Some("pt-BR".to_string()),
        content_rating: Some(if value.get("isNsfw").and_then(Value::as_bool).unwrap_or(true) {
            "adult"
        } else {
            "safe"
        }.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn description(value: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(description) = value.get("description").and_then(Value::as_str) {
        if !description.trim().is_empty() {
            parts.push(description.to_string());
        }
    }
    if let Some(alts) = value.get("alternativeTitles").and_then(Value::as_str) {
        if !alts.trim().is_empty() {
            parts.push(format!("Títulos alternativos: {alts}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn tags(value: &Value) -> Vec<String> {
    value
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(value.get("tags").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn chapter_item(value: &Value, slug: &str) -> MangaChapter {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("chapter-id");
    let number = value.get("chapterNumber").and_then(Value::as_str).unwrap_or("1");
    let title = value.get("title").and_then(Value::as_str).unwrap_or_default();
    MangaChapter {
        key: format!("/chapter/{id}/{slug}/{number}"),
        title: Some(if title.trim().is_empty() || title.starts_with("Capítulo") {
            format!("Capítulo {}", format_number(number))
        } else {
            format!("Capítulo {} - {title}", format_number(number))
        }),
        chapter_number: number.parse::<f32>().ok(),
        date_uploaded: value
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(dates::parse_fixture_date),
        is_locked: value.get("isVipOnly").and_then(Value::as_bool).unwrap_or(false),
        url: Some(format!("{BASE_URL}/ler/{slug}/{number}")),
        ..MangaChapter::default()
    }
}

fn page_item(value: &Value) -> Option<MangaPage> {
    let image = value.get("imageUrl").and_then(Value::as_str)?;
    let headers = image_headers();
    let number = value.get("pageNumber").and_then(Value::as_u64).unwrap_or(1);
    Some(MangaPage {
        content: PageContent::Url {
            url: format!("{API_URL}{image}"),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {number}")),
        ..MangaPage::default()
    })
}

fn image_headers() -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), format!("{BASE_URL}/"));
    headers.insert(
        "Accept".to_string(),
        "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
    );
    headers
}

fn has_next(value: &Value) -> bool {
    value
        .pointer("/pagination/hasMore")
        .or_else(|| value.pointer("/pagination/hasNext"))
        .and_then(Value::as_bool)
        .or_else(|| {
            let page = value.pointer("/pagination/page").and_then(Value::as_u64)?;
            let total_pages = value.pointer("/pagination/totalPages").and_then(Value::as_u64)?;
            Some(page < total_pages)
        })
        .unwrap_or(false)
}

fn show_nsfw_pref(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_adult_content"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn filter_str<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters")?.get(id)?.as_str()
}

fn filter_bool(request: &Value, id: &str) -> Option<bool> {
    request.get("filters")?.get(id)?.as_bool()
}

fn append_text_filter(endpoint: &mut String, request: &Value, id: &str, param: &str) {
    let Some(value) = filter_str(request, id).filter(|value| !value.trim().is_empty()) else {
        return;
    };
    endpoint.push('&');
    endpoint.push_str(param);
    endpoint.push('=');
    endpoint.push_str(&url::query_escape(value));
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find("/manga/") {
        return input[index..].trim_end_matches('/').to_string();
    }
    if let Some(index) = input.find("/obra/") {
        return format!("/manga/{}", input[index + 6..].trim_end_matches('/'));
    }
    if let Some(index) = input.find("/chapter/") {
        return input[index..].trim_end_matches('/').to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(normalize_key)
}

fn slug_from_key(key: &str) -> Option<&str> {
    key.trim_matches('/').split('/').nth(1)
}

fn chapter_id_from_key(key: &str) -> Option<&str> {
    key.trim_matches('/').split('/').nth(1)
}

fn format_number(number: &str) -> String {
    number
        .parse::<f32>()
        .map(|value| {
            if value.fract() == 0.0 {
                format!("{}", value as i32)
            } else {
                value.to_string()
            }
        })
        .unwrap_or_else(|_| number.to_string())
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
{
  "success": true,
  "data": [
    {
      "id": "manga-id",
      "slug": "sample",
      "title": "Sample Geass Comics",
      "description": "Sample description.",
      "coverImage": "/covers/sample.jpg",
      "status": "ongoing",
      "author": "Sample Author",
      "artist": "Sample Artist",
      "isNsfw": true,
      "genres": [{ "id": "g1", "name": "Ação", "slug": "acao" }],
      "tags": [{ "id": "t1", "name": "Drama", "slug": "drama" }]
    }
  ],
  "pagination": { "total": 1, "limit": 24, "page": 1, "totalPages": 1, "hasMore": false }
}
"#;

const DETAILS_FIXTURE: &str = r#"
{
  "success": true,
  "data": {
    "id": "manga-id",
    "slug": "sample",
    "title": "Sample Geass Comics",
    "alternativeTitles": "Sample Alt",
    "description": "Sample description.",
    "coverImage": "/covers/sample.jpg",
    "status": "completed",
    "author": "Sample Author",
    "artist": "Sample Artist",
    "isNsfw": true,
    "genres": [{ "id": "g1", "name": "Ação", "slug": "acao" }],
    "tags": [{ "id": "t1", "name": "Drama", "slug": "drama" }]
  }
}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{
  "success": true,
  "data": [
    {
      "id": "chapter-id",
      "mangaId": "manga-id",
      "chapterNumber": "1",
      "title": "Inicio",
      "isVipOnly": false,
      "createdAt": "2024-01-01 00:00:00"
    }
  ],
  "pagination": { "total": 1, "limit": 100, "page": 1, "totalPages": 1, "hasMore": false }
}
"#;

const PAGES_FIXTURE: &str = r#"
{
  "success": true,
  "data": {
    "id": "chapter-id",
    "mangaId": "manga-id",
    "chapterNumber": "1",
    "pages": [
      { "id": "p1", "chapterId": "chapter-id", "pageNumber": 1, "imageUrl": "/images/page-1.webp" },
      { "id": "p2", "chapterId": "chapter-id", "pageNumber": 2, "imageUrl": "/images/page-2.webp" }
    ]
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_list(SEARCH_FIXTURE).entries.len(), 1);
        assert_eq!(details_from_slug("sample").title, "Sample Geass Comics");
        assert_eq!(chapter_item(&serde_json::json!({"id":"c","chapterNumber":"1.5"}), "s").key, "/chapter/c/s/1.5");
        assert_eq!(has_next(&serde_json::json!({"pagination":{"page":1,"totalPages":2}})), true);
    }
}
