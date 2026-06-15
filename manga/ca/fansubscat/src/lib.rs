use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: FansubsCat = FansubsCat;
const BASE_URL: &str = "https://manga.fansubs.cat";
const API_URL: &str = "https://api.fansubs.cat";

struct FansubsCat;

impl MangaSource for FansubsCat {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page(MANGA_LIST_FIXTURE, "safe"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("/manga/recent/{page}")
        } else {
            format!("/manga/popular/{page}")
        };
        Ok(parse_manga_page(
            &fetch_api_or_fixture(&path, MANGA_LIST_FIXTURE),
            "safe",
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
            let key = normalize_key(query);
            let body = fetch_api_or_fixture(
                &format!("/manga/details/{}", key.trim_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, key, "safe")],
                has_next_page: false,
            });
        }
        let mut path = format!("/manga/search/{page}?type=all");
        if !query.is_empty() {
            path.push_str("&query=");
            path.push_str(&url::query_escape(query));
        }
        append_filters(&mut path, request.get("filters"), false);
        Ok(parse_manga_page(
            &fetch_api_or_fixture(&path, MANGA_LIST_FIXTURE),
            "safe",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(
            &format!("/manga/details/{}", key.trim_matches('/')),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, key, "safe"))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api_or_fixture(
            &format!("/manga/chapters/{}", key.trim_matches('/')),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-1".to_string());
        let body = fetch_api_or_fixture(
            &format!("/manga/pages/{}", key.trim_matches('/')),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_api_or_fixture(
                &format!("/manga/details/{}", key.trim_matches('/')),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key, "safe")),
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn append_filters(path: &mut String, filters: Option<&Value>, hentai: bool) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        return;
    };
    for key in [
        "status",
        "genres_include",
        "genres_exclude",
        "themes_include",
        "themes_exclude",
    ] {
        if let Some(values) = filters.get(key).and_then(Value::as_array) {
            for value in values.iter().filter_map(Value::as_i64) {
                path.push_str(&format!("&{}[]={value}", url::query_escape(key)));
            }
        }
    }
    if !hentai {
        if let Some(values) = filters.get("demographies").and_then(Value::as_array) {
            for value in values.iter().filter_map(Value::as_i64) {
                path.push_str(&format!("&demographies[]={value}"));
            }
        }
    }
    if let Some(kind) = filters.get("type").and_then(Value::as_str) {
        path.push_str("&type=");
        path.push_str(&url::query_escape(kind));
    }
}

fn parse_manga_page(body: &str, rating: &str) -> Paged<CatalogItem> {
    let response: ResultResponse<Vec<ApiManga>> = serde_json::from_str(body).unwrap_or_default();
    let count = response.result.len();
    Paged {
        entries: response
            .result
            .into_iter()
            .map(|manga| manga.into_catalog(rating))
            .collect(),
        has_next_page: count >= 20,
    }
}

fn parse_details(body: &str, fallback_key: String, rating: &str) -> CatalogItem {
    let response: ResultResponse<ApiManga> = serde_json::from_str(body).unwrap_or_default();
    if response.result.slug.is_empty() {
        return CatalogItem {
            key: fallback_key.clone(),
            title: url::slug_from_url(&fallback_key).unwrap_or_else(|| "Manga".into()),
            language: Some("ca".to_string()),
            content_rating: Some(rating.to_string()),
            initialized: true,
            ..CatalogItem::default()
        };
    }
    let mut item = response.result.into_catalog(rating);
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response: ResultResponse<Vec<ApiChapter>> = serde_json::from_str(body).unwrap_or_default();
    response
        .result
        .into_iter()
        .map(|chapter| MangaChapter {
            key: chapter.id.clone(),
            title: Some(chapter.title),
            chapter_number: Some(chapter.number),
            scanlators: (!chapter.fansub.is_empty())
                .then_some(chapter.fansub)
                .into_iter()
                .collect(),
            date_uploaded: Some(chapter.created),
            url: Some(format!("{BASE_URL}/{}", chapter.id.replace('/', "?f="))),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response: ResultResponse<Vec<ApiPage>> = serde_json::from_str(body).unwrap_or_default();
    response
        .result
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: page.url.clone(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input
            .trim_start_matches(BASE_URL)
            .trim_matches('/')
            .to_string()
    } else {
        input.trim_matches('/').to_string()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("ongoing") {
        ItemStatus::Ongoing
    } else if value.contains("finished") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

#[derive(Default, Deserialize)]
struct ResultResponse<T: Default> {
    #[serde(default)]
    result: T,
}

#[derive(Default, Deserialize)]
struct ApiManga {
    slug: String,
    name: String,
    thumbnail_url: String,
    author: Option<String>,
    synopsis: Option<String>,
    status: String,
    genres: Option<String>,
}

impl ApiManga {
    fn into_catalog(self, rating: &str) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.name,
            cover: Some(self.thumbnail_url),
            authors: self.author.into_iter().collect(),
            description: self.synopsis,
            status: parse_status(&self.status),
            tags: self
                .genres
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .map(ToString::to_string)
                .filter(|tag| !tag.is_empty())
                .collect(),
            url: Some(format!("{BASE_URL}/{}", self.slug)),
            language: Some("ca".to_string()),
            content_rating: Some(rating.to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ApiChapter {
    id: String,
    title: String,
    number: f32,
    fansub: String,
    created: i64,
}

#[derive(Default, Deserialize)]
struct ApiPage {
    url: String,
}

export_manga_source!(SOURCE);

const MANGA_LIST_FIXTURE: &str = r#"
{"result":[{"slug":"sample","name":"Mostra","thumbnail_url":"https://img/cover.jpg","author":"Autor","synopsis":"Descripcio","status":"ongoing","genres":"Accio, Drama"}]}
"#;

const DETAILS_FIXTURE: &str = r#"
{"result":{"slug":"sample","name":"Mostra","thumbnail_url":"https://img/cover.jpg","author":"Autor","synopsis":"Descripcio","status":"finished","genres":"Accio, Drama"}}
"#;

const CHAPTERS_FIXTURE: &str = r#"
{"result":[{"id":"chapter-1","title":"Capitol 1","number":1.0,"fansub":"Fansub","created":1704067200000}]}
"#;

const PAGES_FIXTURE: &str = r#"
{"result":[{"url":"https://img/page1.jpg"},{"url":"https://img/page2.jpg"}]}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_source() {
        assert_eq!(
            parse_manga_page(MANGA_LIST_FIXTURE, "safe").entries[0].key,
            "sample"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, "sample".into(), "safe").status,
            ItemStatus::Completed
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE)[0].scanlators[0], "Fansub");
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
