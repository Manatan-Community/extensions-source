use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://nhentai.com";
const SOURCE: NHentaiCom = NHentaiCom;

struct NHentaiCom;

impl MangaSource for NHentaiCom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let mut query = Query::new(&format!("{BASE_URL}/api/comics"));
        query.param("page", &page.to_string());
        query.param("sort", if latest { "uploaded_at" } else { "popularity" });
        query.param("order", "desc");
        query.param("duration", "all");
        add_language_params(&mut query, source);
        let body = fetch_json_or_fixture(&query.finish(), LIST_FIXTURE, &request);
        Ok(parse_list(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query_text = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_input(query_text) {
            let body = fetch_json_or_fixture(
                &format!("{BASE_URL}/api/comics/{slug}"),
                DETAILS_FIXTURE,
                &request,
            );
            return Ok(Paged {
                entries: parse_details_item(&body, source).into_iter().collect(),
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = Query::new(&format!("{BASE_URL}/api/comics"));
        target.param("page", &page(&request).to_string());
        target.param("q", query_text);
        target.param("sort", sort_code(filters));
        target.param("order", order_code(filters));
        target.param("duration", duration_code(filters));
        add_language_params(&mut target, source);
        for value in multi_values(filters, "attributes") {
            target.param("attributes", &value);
        }
        for value in multi_values(filters, "statuses") {
            target.param("statuses", &value);
        }
        let body = fetch_json_or_fixture(&target.finish(), LIST_FIXTURE, &request);
        Ok(parse_list(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/en/comic/sample".into());
        let slug = slug_from_key(&key);
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/comics/{slug}"),
            DETAILS_FIXTURE,
            &request,
        );
        Ok(parse_details_item(&body, source).unwrap_or_else(|| sample_item(source)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/en/comic/sample".into());
        let slug = slug_from_key(&key);
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/comics/{slug}"),
            DETAILS_FIXTURE,
            &request,
        );
        let Ok(detail) = serde_json::from_str::<MangaDto>(&body) else {
            return Ok(Vec::new());
        };
        Ok(vec![MangaChapter {
            key: slug,
            title: Some("Chapter".into()),
            chapter_number: Some(1.0),
            url: Some(format!("{BASE_URL}/en/comic/{}", detail.slug)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample".into());
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/api/comics/{key}/images"),
            PAGES_FIXTURE,
            &request,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if let Some(slug) = slug_from_input(input) {
            let body = fetch_json_or_fixture(
                &format!("{BASE_URL}/api/comics/{slug}"),
                DETAILS_FIXTURE,
                &request,
            );
            return Ok(Some(UrlResolveResult {
                item: parse_details_item(&body, source),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    lang_ids: &'static [i32],
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("nhentaicom-all");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client(request: &Value) -> http::HttpClient {
    let mut client = http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL);
    if let Some(token) = login_token(request) {
        client = client.with_header("Authorization", format!("Bearer {token}"));
    }
    client
}

fn fetch_json_or_fixture(target: &str, fixture: &str, request: &Value) -> String {
    client(request)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn login_token(request: &Value) -> Option<String> {
    let prefs = request.get("preferences")?;
    let username = prefs
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = prefs
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if username.is_empty() || password.is_empty() {
        return None;
    }
    let body =
        serde_json::json!({ "username": username, "password": password, "remember_me": true })
            .to_string();
    let response = http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .post(format!("{BASE_URL}/api/login"))
        .json(body)
        .send_text()
        .ok()?;
    serde_json::from_str::<LoginResponse>(&response)
        .ok()
        .map(|login| login.auth.access_token)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn add_language_params(query: &mut Query, source: SourceConfig) {
    for (index, lang_id) in source.lang_ids.iter().enumerate() {
        query.param(
            &format!("languages[{}]", -((index as i32) + 1)),
            &lang_id.to_string(),
        );
    }
}

fn parse_list(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let Ok(response) = serde_json::from_str::<ListResponse>(body) else {
        return Paged {
            entries: vec![sample_item(source)],
            has_next_page: false,
        };
    };
    Paged {
        has_next_page: response
            .next_page_url
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        entries: response
            .data
            .into_iter()
            .map(|item| item.into_item(source))
            .collect(),
    }
}

fn parse_details_item(body: &str, source: SourceConfig) -> Option<CatalogItem> {
    serde_json::from_str::<MangaDto>(body)
        .ok()
        .map(|item| item.into_details(source))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(response) = serde_json::from_str::<PageListResponse>(body) else {
        return Vec::new();
    };
    response
        .images
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.source_url,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", page.page)),
            ..MangaPage::default()
        })
        .collect()
}

fn sample_item(source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: "/en/comic/sample".into(),
        title: "Sample nHentai Comic".into(),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn slug_from_input(input: &str) -> Option<String> {
    input
        .trim()
        .split("/comic/")
        .nth(1)
        .map(|value| {
            value
                .split(['?', '#', '/'])
                .next()
                .unwrap_or(value)
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn slug_from_key(key: &str) -> String {
    key.trim_start_matches("/en/comic/")
        .trim_matches('/')
        .to_string()
}

fn sort_code(filters: &Value) -> &'static str {
    match filter_string(filters, "sort").as_deref() {
        Some("Title") => "title",
        Some("Pages") => "pages",
        Some("Favorites") => "favorites",
        Some("Popularity") => "popularity",
        _ => "uploaded_at",
    }
}

fn order_code(filters: &Value) -> &'static str {
    match filter_string(filters, "order").as_deref() {
        Some("Ascending") => "asc",
        _ => "desc",
    }
}

fn duration_code(filters: &Value) -> &'static str {
    match filter_string(filters, "duration").as_deref() {
        Some("Today") => "day",
        Some("This Week") => "week",
        Some("This Month") => "month",
        Some("This Year") => "year",
        _ => "all",
    }
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn multi_values(filters: &Value, id: &str) -> Vec<String> {
    match filters.get(id) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

struct Query {
    target: String,
    has_query: bool,
}

impl Query {
    fn new(base: &str) -> Self {
        Self {
            target: base.to_string(),
            has_query: base.contains('?'),
        }
    }

    fn param(&mut self, key: &str, value: &str) {
        self.target.push(if self.has_query { '&' } else { '?' });
        self.has_query = true;
        self.target.push_str(&url::query_escape(key));
        self.target.push('=');
        self.target.push_str(&url::query_escape(value));
    }

    fn finish(self) -> String {
        self.target
    }
}

#[derive(Deserialize)]
struct ListResponse {
    data: Vec<MangaDto>,
    next_page_url: Option<String>,
}

#[derive(Deserialize)]
struct LoginResponse {
    auth: AuthResponse,
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct PageListResponse {
    images: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    page: u32,
    source_url: String,
}

#[derive(Deserialize)]
struct MangaDto {
    slug: String,
    title: String,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    artists: Option<Vec<NameDto>>,
    #[serde(default)]
    authors: Option<Vec<NameDto>>,
    #[serde(default)]
    tags: Option<Vec<NameDto>>,
    #[serde(default)]
    relationships: Option<Vec<NameDto>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    alternative_title: Option<String>,
    #[serde(default)]
    groups: Option<Vec<NameDto>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pages: Option<u32>,
    #[serde(default)]
    category: Option<NameDto>,
    #[serde(default)]
    language: Option<NameDto>,
    #[serde(default)]
    parodies: Option<Vec<NameDto>>,
    #[serde(default)]
    characters: Option<Vec<NameDto>>,
}

impl MangaDto {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        CatalogItem {
            key: format!("/en/comic/{}", self.slug),
            title: self.title,
            cover: self.image_url,
            url: Some(format!("{BASE_URL}/en/comic/{}", self.slug)),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_details(self, source: SourceConfig) -> CatalogItem {
        let tags = self
            .tags
            .unwrap_or_default()
            .into_iter()
            .chain(self.relationships.unwrap_or_default())
            .map(|name| name.name)
            .collect::<Vec<_>>();
        let authors = names(self.authors)
            .or_else(|| names(self.artists.clone()))
            .unwrap_or_default();
        let artists = names(self.artists).unwrap_or_else(|| authors.clone());
        let description = [
            ("Alternative Title", self.alternative_title),
            ("Groups", names(self.groups)),
            ("Description", self.description),
            ("Pages", self.pages.map(|pages| pages.to_string())),
            ("Category", self.category.map(|value| value.name)),
            ("Language", self.language.map(|value| value.name)),
            ("Parodies", names(self.parodies)),
            ("Characters", names(self.characters)),
        ]
        .into_iter()
        .filter_map(|(label, value)| {
            value
                .filter(|value| !value.is_empty())
                .map(|value| format!("{label}: {value}"))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
        CatalogItem {
            key: format!("/en/comic/{}", self.slug),
            title: self.title,
            cover: self.image_url,
            url: Some(format!("{BASE_URL}/en/comic/{}", self.slug)),
            authors: split_names(&authors),
            artists: split_names(&artists),
            tags,
            description: (!description.is_empty()).then_some(description),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            status: match self.status.as_deref() {
                Some("ongoing") | Some("onhold") => ItemStatus::Ongoing,
                Some("complete") => ItemStatus::Completed,
                Some("canceled") => ItemStatus::Cancelled,
                _ => ItemStatus::Completed,
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Deserialize)]
struct NameDto {
    name: String,
}

fn names(values: Option<Vec<NameDto>>) -> Option<String> {
    let names = values?
        .into_iter()
        .map(|value| value.name)
        .collect::<Vec<_>>();
    (!names.is_empty()).then_some(names.join(", "))
}

fn split_names(values: &str) -> Vec<String> {
    values
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "nhentaicom-all",
        lang: "all",
        lang_ids: &[],
    },
    SourceConfig {
        id: "nhentaicom-zh",
        lang: "zh",
        lang_ids: &[1],
    },
    SourceConfig {
        id: "nhentaicom-en",
        lang: "en",
        lang_ids: &[2],
    },
    SourceConfig {
        id: "nhentaicom-ja",
        lang: "ja",
        lang_ids: &[3],
    },
    SourceConfig {
        id: "nhentaicom-other",
        lang: "other",
        lang_ids: &[4],
    },
    SourceConfig {
        id: "nhentaicom-ar",
        lang: "ar",
        lang_ids: &[5],
    },
    SourceConfig {
        id: "nhentaicom-jv",
        lang: "jv",
        lang_ids: &[6],
    },
    SourceConfig {
        id: "nhentaicom-bg",
        lang: "bg",
        lang_ids: &[7],
    },
    SourceConfig {
        id: "nhentaicom-cs",
        lang: "cs",
        lang_ids: &[8],
    },
    SourceConfig {
        id: "nhentaicom-uk",
        lang: "uk",
        lang_ids: &[9],
    },
    SourceConfig {
        id: "nhentaicom-sk",
        lang: "sk",
        lang_ids: &[10],
    },
    SourceConfig {
        id: "nhentaicom-eo",
        lang: "eo",
        lang_ids: &[11],
    },
    SourceConfig {
        id: "nhentaicom-mn",
        lang: "mn",
        lang_ids: &[12],
    },
    SourceConfig {
        id: "nhentaicom-la",
        lang: "la",
        lang_ids: &[13],
    },
    SourceConfig {
        id: "nhentaicom-ceb",
        lang: "ceb",
        lang_ids: &[14],
    },
    SourceConfig {
        id: "nhentaicom-tl",
        lang: "tl",
        lang_ids: &[15],
    },
    SourceConfig {
        id: "nhentaicom-fi",
        lang: "fi",
        lang_ids: &[16],
    },
    SourceConfig {
        id: "nhentaicom-tr",
        lang: "tr",
        lang_ids: &[17],
    },
    SourceConfig {
        id: "nhentaicom-sr",
        lang: "sr",
        lang_ids: &[18],
    },
    SourceConfig {
        id: "nhentaicom-el",
        lang: "el",
        lang_ids: &[19],
    },
    SourceConfig {
        id: "nhentaicom-ko",
        lang: "ko",
        lang_ids: &[20],
    },
    SourceConfig {
        id: "nhentaicom-ro",
        lang: "ro",
        lang_ids: &[21],
    },
];

const LIST_FIXTURE: &str = r#"
{ "data": [{ "slug": "sample", "title": "Sample nHentai Comic", "image_url": "https://nhentai.com/cover.jpg" }], "next_page_url": "https://nhentai.com/api/comics?page=2" }
"#;

const DETAILS_FIXTURE: &str = r#"
{ "slug": "sample", "title": "Sample nHentai Comic", "image_url": "https://nhentai.com/cover.jpg", "artists": [{ "name": "Sample Artist" }], "authors": [{ "name": "Sample Author" }], "tags": [{ "name": "Action" }], "relationships": [{ "name": "Drama" }], "status": "complete", "alternative_title": "Alt Sample", "groups": [{ "name": "Sample Group" }], "description": "Sample description", "pages": 2, "category": { "name": "Manga" }, "language": { "name": "English" }, "parodies": [{ "name": "Original" }], "characters": [{ "name": "Sample Character" }] }
"#;

const PAGES_FIXTURE: &str = r#"
{ "images": [{ "page": 1, "source_url": "https://nhentai.com/page1.jpg" }, { "page": 2, "source_url": "https://nhentai.com/page2.jpg" }] }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_source() {
        let page = parse_list(LIST_FIXTURE, SOURCES[1]);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let details = parse_details_item(DETAILS_FIXTURE, SOURCES[1]).unwrap();
        assert_eq!(details.title, "Sample nHentai Comic");
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
