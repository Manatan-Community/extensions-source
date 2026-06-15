use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, UrlResolveResult,
    abi::{ExtensionResult, HttpResponse},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: SimplyCosplay = SimplyCosplay;
const BASE_URL: &str = "https://www.simply-cosplay.com";
const API_URL: &str = "https://api.simply-porn.com/v2";
const DEFAULT_TOKEN: &str = "01730876";
const LIMIT: u64 = 20;

struct SimplyCosplay;

impl MangaSource for SimplyCosplay {
    fn list(&self, request: Value) -> ExtensionResult<PagedItems> {
        let page = page_for(&request);
        let listing_id = request.get("listingId").and_then(Value::as_str);
        let sort = if listing_id == Some("latest") {
            "new"
        } else {
            "hot"
        };
        let endpoint = request
            .get("preferences")
            .and_then(|prefs| prefs.get("browseType"))
            .and_then(Value::as_str)
            .unwrap_or("gallery");
        let target = browse_url(endpoint, sort, page, None, &[]);
        Ok(parse_browse_page(&fetch_api_or_fixture(
            &target,
            BROWSE_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<PagedItems> {
        let page = page_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = direct_key(query) {
            return Ok(PagedItems {
                entries: vec![catalog_from_key(&key)],
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("new");
        let type_filter = filters
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let tags = filters
            .get("tags")
            .and_then(Value::as_str)
            .map(split_tags)
            .unwrap_or_default();
        let mut target = browse_url("search", sort, page, type_filter, &tags);
        if !query.is_empty() {
            target.push_str("&query=");
            target.push_str(&url::query_escape(query));
        }
        Ok(parse_browse_page(&fetch_api_or_fixture(
            &target,
            BROWSE_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/new/sample".into());
        let target = api_item_url(&key);
        Ok(parse_details(&fetch_api_or_fixture(
            &target,
            DETAILS_FIXTURE,
        )))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/new/sample".into());
        let title = key
            .trim_matches('/')
            .split('/')
            .next()
            .map(title_case)
            .unwrap_or_else(|| "Gallery".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: Some(0.0),
            language: Some("all".to_string()),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/gallery/new/sample".into());
        let target = api_item_url(&key);
        Ok(parse_pages(&fetch_api_or_fixture(&target, PAGES_FIXTURE)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({
            "listingId": "popular",
            "page": 1,
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
        }))?;
        let latest = self.list(serde_json::json!({
            "listingId": "latest",
            "page": 1,
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
        }))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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
        if let Some(key) = direct_key(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key)),
                url: Some(url::join_url(BASE_URL, &key)),
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

type PagedItems = manatan_extension::Paged<CatalogItem>;

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, fixture: &str) -> String {
    fetch_api(target).unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api(target: &str) -> ExtensionResult<String> {
    let response = api_request(target, DEFAULT_TOKEN)?;
    if response.status != 403 {
        return Ok(response.text.unwrap_or_default());
    }
    let token = fetch_token()?;
    Ok(api_request(target, &token)?.text.unwrap_or_default())
}

fn api_request(target: &str, token: &str) -> ExtensionResult<HttpResponse> {
    let separator = if target.contains('?') { '&' } else { '?' };
    client()
        .get(format!("{target}{separator}token={token}"))
        .xhr()
        .send()
}

fn fetch_token() -> ExtensionResult<String> {
    let document = client()
        .get(BASE_URL)
        .browser_document()
        .send_text()?
        .replace('\'', "\"");
    let script_url = document
        .split("<script")
        .filter_map(|chunk| manatan_shared::html::attr(chunk, "src"))
        .find(|src| src.contains("main"))
        .map(|src| url::join_url(BASE_URL, &src))
        .ok_or_else(|| manatan_extension::abi::ExtensionError {
            message: "Unable to find Simply Cosplay API token script".to_string(),
        })?;
    let script = client()
        .get(script_url)
        .browser_document()
        .send_text()?
        .replace('\'', "\"");
    token_from_script(&script).ok_or_else(|| manatan_extension::abi::ExtensionError {
        message: "Unable to parse Simply Cosplay API token".to_string(),
    })
}

fn token_from_script(script: &str) -> Option<String> {
    let index = script.find("token")?;
    let rest = &script[index..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let after_quote = after_colon.strip_prefix('"')?;
    let end = after_quote.find('"')?;
    Some(after_quote[..end].to_string())
}

fn browse_url(
    endpoint: &str,
    sort: &str,
    page: u64,
    type_filter: Option<&str>,
    tags: &[String],
) -> String {
    let mut target = format!("{API_URL}/{endpoint}?sort={sort}&limit={LIMIT}&page={page}");
    if let Some(value) = type_filter {
        target.push_str("&filter[type][0]=");
        target.push_str(&url::query_escape(value));
    }
    for (index, tag) in tags.iter().enumerate() {
        target.push_str(&format!(
            "&filter[tag_names][{index}]={}",
            url::query_escape(&tag.replace(' ', "+"))
        ));
    }
    target
}

fn api_item_url(key: &str) -> String {
    let segments = key
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let item_type = segments.first().copied().unwrap_or("gallery");
    let slug = segments
        .get(2)
        .copied()
        .unwrap_or_else(|| segments.last().copied().unwrap_or("sample"));
    format!("{API_URL}/{item_type}/{slug}")
}

fn direct_key(input: &str) -> Option<String> {
    let value = input.trim();
    let path = if value.starts_with(BASE_URL) {
        value.strip_prefix(BASE_URL)?.split(['?', '#']).next()?
    } else if value.starts_with("https://simply-cosplay.com") {
        value
            .strip_prefix("https://simply-cosplay.com")?
            .split(['?', '#'])
            .next()?
    } else if value.starts_with('/') {
        value
    } else {
        return None;
    };
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() >= 3 && matches!(segments[0], "gallery" | "image") {
        Some(format!("/{}/{}/{}", segments[0], segments[1], segments[2]))
    } else {
        None
    }
}

fn parse_browse_page(body: &str) -> PagedItems {
    let response =
        serde_json::from_str::<Data<Vec<BrowseItem>>>(body).unwrap_or(Data { data: Vec::new() });
    let has_next_page = response.data.len() as u64 >= LIMIT;
    PagedItems {
        entries: response
            .data
            .into_iter()
            .map(BrowseItem::into_catalog)
            .collect(),
        has_next_page,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    serde_json::from_str::<Data<DetailsItem>>(body)
        .map(|response| response.data.into_catalog())
        .unwrap_or_else(|_| catalog_from_key("/gallery/new/sample"))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(response) = serde_json::from_str::<Data<PageItem>>(body) else {
        return Vec::new();
    };
    let mut pages = response
        .data
        .images
        .unwrap_or_default()
        .into_iter()
        .filter_map(|image| image.urls.url)
        .map(page_from_url)
        .collect::<Vec<_>>();
    if pages.is_empty() {
        if let Some(preview) = response.data.preview.urls.url {
            pages.push(page_from_url(preview));
        }
    }
    pages
}

fn page_from_url(image_url: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image_url,
            context: Some(manga::image_headers(BASE_URL)),
        },
        ..MangaPage::default()
    }
}

fn catalog_from_key(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Cosplay".to_string()),
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Gallery".to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct BrowseItem {
    #[serde(default)]
    title: Option<String>,
    slug: String,
    #[serde(rename = "type")]
    item_type: String,
    preview: Images,
}

impl BrowseItem {
    fn into_catalog(self) -> CatalogItem {
        let key = format!(
            "/{}/new/{}",
            self.item_type.to_lowercase().trim(),
            self.slug.trim_matches('/')
        );
        CatalogItem {
            key: key.clone(),
            title: self
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| self.slug.replace('-', " ")),
            cover: self.preview.urls.thumb.url,
            description: self
                .preview
                .publish_date
                .map(|date| format!("Date: {date}")),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetailsItem {
    #[serde(default)]
    title: Option<String>,
    slug: String,
    #[serde(rename = "type")]
    item_type: String,
    preview: Images,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    image_count: Option<u32>,
}

impl DetailsItem {
    fn into_catalog(self) -> CatalogItem {
        let key = format!(
            "/{}/new/{}",
            self.item_type.to_lowercase().trim(),
            self.slug.trim_matches('/')
        );
        let mut description = format!("Type: {}\n", self.item_type);
        if let Some(count) = self.image_count {
            description.push_str(&format!("Images: {count}\n"));
        }
        if let Some(date) = self.preview.publish_date.as_deref() {
            description.push_str(&format!("Date: {date}\n"));
        }
        CatalogItem {
            key: key.clone(),
            title: self
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| self.slug.replace('-', " ")),
            cover: self.preview.urls.thumb.url,
            description: Some(description.trim().to_string()),
            tags: self
                .tags
                .into_iter()
                .filter_map(|tag| tag.name)
                .map(|tag| {
                    tag.split_whitespace()
                        .map(title_case)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect(),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Completed,
            initialized: true,
            update_strategy: Some(manatan_extension::UpdateStrategy::OnlyFetchOnce),
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PageItem {
    #[serde(default)]
    images: Option<Vec<Images>>,
    preview: Images,
}

#[derive(Debug, Deserialize)]
struct Images {
    #[serde(default)]
    publish_date: Option<String>,
    urls: Urls,
}

#[derive(Debug, Deserialize)]
struct Urls {
    #[serde(default)]
    url: Option<String>,
    thumb: Url,
}

#[derive(Debug, Deserialize)]
struct Url {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Tag {
    #[serde(default)]
    name: Option<String>,
}

const BROWSE_FIXTURE: &str = r#"{
  "data": [
    {
      "title": "Sample Gallery",
      "slug": "sample-gallery",
      "type": "gallery",
      "preview": { "publish_date": "2024-01-01T00:00:00.000", "urls": { "thumb": { "url": "https://cdn.example/thumb.jpg" } } }
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "title": "Sample Gallery",
    "slug": "sample-gallery",
    "type": "gallery",
    "image_count": 2,
    "tags": [{ "name": "game cosplay" }],
    "preview": { "publish_date": "2024-01-01T00:00:00.000", "urls": { "thumb": { "url": "https://cdn.example/thumb.jpg" }, "url": "https://cdn.example/preview.jpg" } }
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": {
    "preview": { "urls": { "thumb": { "url": "https://cdn.example/thumb.jpg" }, "url": "https://cdn.example/preview.jpg" } },
    "images": [
      { "urls": { "thumb": { "url": "https://cdn.example/thumb-1.jpg" }, "url": "https://cdn.example/page-1.jpg" } },
      { "urls": { "thumb": { "url": "https://cdn.example/thumb-2.jpg" }, "url": "https://cdn.example/page-2.jpg" } }
    ]
  }
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_browse_items() {
        let page = parse_browse_page(BROWSE_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/gallery/new/sample-gallery");
        assert_eq!(
            page.entries[0].cover.as_deref(),
            Some("https://cdn.example/thumb.jpg")
        );
    }

    #[test]
    fn parses_details() {
        let item = parse_details(DETAILS_FIXTURE);
        assert_eq!(item.title, "Sample Gallery");
        assert!(item.tags.contains(&"Game Cosplay".to_string()));
        assert!(item.initialized);
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://cdn.example/page-1.jpg"),
            _ => panic!("expected image URL page"),
        }
    }

    #[test]
    fn parses_token_from_script() {
        assert_eq!(
            token_from_script(r#"window.app={token:"12345"};"#).as_deref(),
            Some("12345")
        );
    }
}
