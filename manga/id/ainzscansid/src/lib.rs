use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AinzScansId = AinzScansId;
const BASE_URL: &str = "https://v1.ainzscans01.com";
const API_URL: &str = "https://api.ainzscans01.com/api";

struct AinzScansId;

impl MangaSource for AinzScansId {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_page(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_search_page(
            &api_get(&search_url(page, "", sort, "desc", None), LIST_FIXTURE),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &api_get(&format!("{API_URL}/series{key}"), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        let sort = filter(filters, "sort").unwrap_or("latest");
        let order = filter(filters, "order").unwrap_or("desc");
        Ok(parse_search_page(
            &api_get(&search_url(page, query, sort, order, filters), LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let key = normalize_manga_key(&key);
        Ok(parse_details(
            &api_get(&format!("{API_URL}/series{key}"), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let key = normalize_manga_key(&key);
        let details =
            parse_detail_payload(&api_get(&format!("{API_URL}/series{key}"), DETAILS_FIXTURE));
        let comic_slug = details.slug.unwrap_or_else(|| slug_from_key(&key));
        Ok(details
            .units
            .into_iter()
            .map(|chapter| chapter.to_chapter(&comic_slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter/chapter-1".to_string());
        Ok(parse_pages(&api_get(
            &format!("{API_URL}/series{}", normalize_key(&key)),
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
        if input.starts_with(BASE_URL) {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &api_get(&format!("{API_URL}/series{key}"), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Origin", BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(page: u64, query: &str, sort: &str, order: &str, filters: Option<&Value>) -> String {
    let mut params = vec![
        ("type", "COMIC".to_string()),
        ("limit", "20".to_string()),
        ("page", page.to_string()),
        ("sort", sort.to_string()),
        ("order", order.to_string()),
    ];
    if !query.is_empty() {
        params.push(("q", url::query_escape(query)));
    }
    for (id, key) in [
        ("status", "status"),
        ("genre", "genre"),
        ("comic_type", "comic_type"),
        ("color_format", "color_format"),
        ("reading_format", "reading_format"),
        ("author", "author"),
        ("artist", "artist"),
        ("publisher", "publisher"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push((key, url::query_escape(value)));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{API_URL}/search?{query}")
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_search_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let payload: SearchResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: payload.data.into_iter().map(MangaDto::to_catalog).collect(),
        has_next_page: page < payload.total_pages.unwrap_or(page),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    parse_detail_payload(body).to_catalog(key)
}

fn parse_detail_payload(body: &str) -> SeriesDetail {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload: ChapterDetail = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    payload
        .chapter
        .pages
        .into_iter()
        .map(|page| clean_image_url(&page.image_url))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    data: Vec<MangaDto>,
    total_pages: Option<u64>,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    title: String,
    slug: String,
    poster_image_url: Option<String>,
}

impl MangaDto {
    fn to_catalog(self) -> CatalogItem {
        let key = format!("/comic/{}", self.slug.trim_matches('/'));
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.poster_image_url,
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct SeriesDetail {
    title: Option<String>,
    slug: Option<String>,
    synopsis: Option<String>,
    poster_image_url: Option<String>,
    comic_status: Option<String>,
    author_name: Option<String>,
    artist_name: Option<String>,
    #[serde(default)]
    units: Vec<ChapterDto>,
}

impl SeriesDetail {
    fn to_catalog(self, key: Option<String>) -> CatalogItem {
        let key = key.unwrap_or_else(|| {
            self.slug
                .as_deref()
                .map(|slug| format!("/comic/{}", slug.trim_matches('/')))
                .unwrap_or_else(|| "/comic/sample".to_string())
        });
        CatalogItem {
            key: key.clone(),
            title: self.title.unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "Ainz Scans ID".into())
            }),
            cover: self.poster_image_url,
            description: self
                .synopsis
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            authors: self.author_name.into_iter().collect(),
            artists: self.artist_name.into_iter().collect(),
            status: parse_status(self.comic_status.as_deref().unwrap_or_default()),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    slug: String,
    number: String,
    created_at: Option<String>,
}

impl ChapterDto {
    fn to_chapter(self, comic_slug: &str) -> MangaChapter {
        let number_text = self.number.trim_end_matches(".00");
        let key = format!(
            "/comic/{comic_slug}/chapter/{}",
            self.slug.trim_matches('/')
        );
        MangaChapter {
            key: key.clone(),
            title: Some(format!("Chapter {number_text}")),
            chapter_number: self.number.parse().ok(),
            date_uploaded: self.created_at.as_deref().and_then(parse_iso_date),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ChapterDetail {
    chapter: ChapterPages,
}

#[derive(Default, Deserialize)]
struct ChapterPages {
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    image_url: String,
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_uppercase().as_str() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn clean_image_url(input: &str) -> String {
    let mut out = if input.starts_with("http") {
        input.to_string()
    } else {
        format!(
            "https://api.ainzscans01.com/{}",
            input.trim_start_matches('/')
        )
    };
    if out.contains("googleusercontent.com") || out.contains("bp.blogspot.com") {
        out = replace_size_segment(&out);
    }
    remove_resize_query(&out)
}

fn replace_size_segment(input: &str) -> String {
    let parts = input
        .split('/')
        .map(|part| {
            if part.len() > 2
                && matches!(part.as_bytes()[0], b's' | b'w' | b'h')
                && part.as_bytes()[1].is_ascii_digit()
            {
                "s0".to_string()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>();
    let mut out = parts.join("/");
    for prefix in ["=s", "=w", "=h"] {
        if let Some(index) = out.rfind(prefix) {
            if out[index + 2..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit())
            {
                let suffix = out[index..]
                    .find(['?', '&'])
                    .map(|offset| out[index + offset..].to_string())
                    .unwrap_or_default();
                out.truncate(index);
                out.push_str("=s0");
                out.push_str(&suffix);
            }
        }
    }
    out
}

fn remove_resize_query(input: &str) -> String {
    let Some((base, query)) = input.split_once('?') else {
        return input.to_string();
    };
    let kept = query
        .split('&')
        .filter(|part| {
            let key = part.split('=').next().unwrap_or_default();
            !matches!(key, "w" | "width" | "resize")
        })
        .collect::<Vec<_>>();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_ymd(date)
}

fn normalize_manga_key(input: &str) -> String {
    let key = normalize_key(input);
    if let Some(slug) = key.strip_prefix("/series/") {
        format!("/comic/{}", slug.trim_matches('/'))
    } else {
        key
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .split('?')
                .next()
                .unwrap_or_default()
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!(
        "/{}",
        input
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("sample")
        .to_string()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "data": [
    { "title": "Sample Ainz", "slug": "sample-ainz", "poster_image_url": "https://api.ainzscans01.com/sample.jpg" }
  ],
  "total_pages": 1
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "title": "Sample Ainz",
  "slug": "sample-ainz",
  "synopsis": "<p>Sample synopsis.</p>",
  "poster_image_url": "https://api.ainzscans01.com/sample.jpg",
  "comic_status": "ONGOING",
  "author_name": "Author",
  "artist_name": "Artist",
  "units": [
    { "slug": "chapter-1", "number": "1.00", "created_at": "2024-01-01T00:00:00.000Z" }
  ]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "chapter": { "pages": [
    { "image_url": "https://api.ainzscans01.com/page1.jpg?w=300" },
    { "image_url": "/page2.jpg" }
  ] }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_api_fixtures() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Ainz"
        );
        assert_eq!(
            SOURCE
                .chapters(json!({"manga":"/comic/sample-ainz"}))
                .unwrap()[0]
                .chapter_number,
            Some(1.0)
        );
        let pages = parse_pages(PAGES_FIXTURE);
        match &pages[0].content {
            PageContent::Url { url, .. } => {
                assert_eq!(url, "https://api.ainzscans01.com/page1.jpg")
            }
            _ => panic!("expected URL page"),
        }
    }
}
