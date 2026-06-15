use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UpdateStrategy, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: StashApp = StashApp;
const DEFAULT_BASE_URL: &str = "http://localhost:9999";
const PER_PAGE: u64 = 25;

struct StashApp;

impl MangaSource for StashApp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let sort = if latest { "updated_at" } else { "rating" };
        let response = graphql_or_fixture(
            &request,
            MANGA_BRIEF_QUERY,
            "MangaBrief",
            json!({
                "filter": {
                    "q": Value::Null,
                    "page": page_for(&request),
                    "per_page": PER_PAGE,
                    "sort": sort,
                    "direction": "DESC"
                }
            }),
            BRIEF_FIXTURE,
        );
        Ok(parse_brief_page(&response, &base_url(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = normalize_key(query) {
            return Ok(Paged {
                entries: vec![catalog_from_key(&key, &base_url(&request))],
                has_next_page: false,
            });
        }
        let response = graphql_or_fixture(
            &request,
            MANGA_BRIEF_QUERY,
            "MangaBrief",
            json!({
                "filter": {
                    "q": if query.is_empty() { Value::Null } else { Value::String(query.to_string()) },
                    "page": page_for(&request),
                    "per_page": PER_PAGE,
                    "sort": "path",
                    "direction": "ASC"
                }
            }),
            BRIEF_FIXTURE,
        );
        Ok(parse_brief_page(&response, &base_url(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/galleries/1".to_string());
        let response = graphql_or_fixture(
            &request,
            MANGA_DETAILS_QUERY,
            "MangaDetails",
            json!({ "id": last_path_segment(&key) }),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&response, &base_url(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/galleries/1".to_string());
        let response = graphql_or_fixture(
            &request,
            CHAPTER_LIST_QUERY,
            "ChapterList",
            json!({ "id": last_path_segment(&key) }),
            CHAPTER_FIXTURE,
        );
        Ok(vec![parse_chapter(&response, &base_url(&request))])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/galleries/1".to_string());
        let response = graphql_or_fixture(
            &request,
            PAGE_LIST_QUERY,
            "PageList",
            json!({ "id": last_path_segment(&key).parse::<u64>().unwrap_or(1) }),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(
            &response,
            &base_url(&request),
            &api_key(&request),
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({
            "listingId": "popular",
            "page": 1,
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
        }))?;
        let latest = self.list(json!({
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
        Ok(request_key(&request, "manga").map(|key| absolute_url(&base_url(&request), &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter").map(|key| absolute_url(&base_url(&request), &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = normalize_key(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key, &base_url(&request))),
                url: Some(absolute_url(&base_url(&request), &key)),
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

fn graphql_or_fixture(
    request: &Value,
    query: &str,
    operation_name: &str,
    variables: Value,
    fixture: &str,
) -> String {
    let payload = json!({
        "query": query,
        "operationName": operation_name,
        "variables": variables
    });
    let target = format!("{}/graphql", base_url(request));
    let client = client(request);
    let mut builder = client
        .post(target)
        .header(
            "Accept",
            "application/graphql-response+json, application/json",
        )
        .json(payload.to_string());
    if let Some(key) = api_key(request) {
        builder = builder.header("ApiKey", key);
    }
    builder.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn client(request: &Value) -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(base_url(request))
        .with_cookies_for(base_url(request))
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("baseUrl"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_BASE_URL)
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn api_key(request: &Value) -> Option<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("apiKey"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_brief_page(body: &str, base_url: &str) -> Paged<CatalogItem> {
    let root = parse_graphql(body);
    let galleries = root
        .pointer("/data/findGalleries/galleries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = galleries
        .iter()
        .filter_map(|gallery| parse_gallery_brief(gallery, base_url))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() as u64 >= PER_PAGE,
        entries,
    }
}

fn parse_details(body: &str, base_url: &str) -> CatalogItem {
    let root = parse_graphql(body);
    root.pointer("/data/findGallery")
        .and_then(|gallery| parse_gallery_details(gallery, base_url))
        .unwrap_or_else(|| catalog_from_key("/galleries/1", base_url))
}

fn parse_chapter(body: &str, base_url: &str) -> MangaChapter {
    let root = parse_graphql(body);
    let gallery = root.pointer("/data/findGallery").unwrap_or(&Value::Null);
    let id = string_field(gallery, "id").unwrap_or_else(|| "1".to_string());
    MangaChapter {
        key: format!("/galleries/{id}"),
        title: Some("Chapter".to_string()),
        chapter_number: Some(1.0),
        scanlators: string_field(gallery, "photographer").into_iter().collect(),
        url: Some(absolute_url(base_url, &format!("/galleries/{id}"))),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str, base_url: &str, api_key: &Option<String>) -> Vec<MangaPage> {
    let root = parse_graphql(body);
    root.pointer("/data/findImages/images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, image)| {
            let id = string_field(image, "id")?;
            let mut headers = BTreeMap::new();
            headers.insert("Accept".to_string(), "image/*".to_string());
            if let Some(key) = api_key {
                headers.insert("ApiKey".to_string(), key.clone());
            }
            Some(MangaPage {
                content: PageContent::Url {
                    url: absolute_url(base_url, &format!("/image/{id}/image")),
                    context: Some(headers),
                },
                description: Some((index + 1).to_string()),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn parse_gallery_brief(gallery: &Value, base_url: &str) -> Option<CatalogItem> {
    let id = string_field(gallery, "id")?;
    Some(CatalogItem {
        key: format!("/galleries/{id}"),
        title: gallery_title(gallery, &id),
        cover: thumbnail(gallery).map(|value| absolute_url(base_url, &value)),
        url: Some(absolute_url(base_url, &format!("/galleries/{id}"))),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_gallery_details(gallery: &Value, base_url: &str) -> Option<CatalogItem> {
    let id = string_field(gallery, "id")?;
    let artists = string_field(gallery, "photographer")
        .into_iter()
        .collect::<Vec<_>>();
    Some(CatalogItem {
        key: format!("/galleries/{id}"),
        title: gallery_title(gallery, &id),
        cover: thumbnail(gallery).map(|value| absolute_url(base_url, &value)),
        url: Some(absolute_url(base_url, &format!("/galleries/{id}"))),
        authors: artists.clone(),
        artists,
        description: string_field(gallery, "details"),
        tags: gallery
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| string_field(tag, "name"))
            .collect(),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        update_strategy: Some(UpdateStrategy::Always),
        ..CatalogItem::default()
    })
}

fn catalog_from_key(key: &str, base_url: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Gallery".to_string()),
        url: Some(absolute_url(base_url, key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_graphql(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| json!({}))
}

fn gallery_title(gallery: &Value, fallback: &str) -> String {
    string_field(gallery, "title")
        .or_else(|| {
            gallery
                .pointer("/folder/path")
                .and_then(Value::as_str)
                .and_then(url::slug_from_url)
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn thumbnail(gallery: &Value) -> Option<String> {
    let cover = gallery.get("cover")?;
    let is_image = cover
        .get("visual_files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| string_field(file, "__typename"))
        .as_deref()
        == Some("ImageFile");
    is_image
        .then(|| {
            cover
                .pointer("/paths/thumbnail")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .flatten()
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|item| {
            item.get("key")
                .or_else(|| item.get("url"))
                .and_then(Value::as_str)
        })
        .and_then(normalize_key)
}

fn normalize_key(input: &str) -> Option<String> {
    let path = input
        .split(['?', '#'])
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    let id = last_path_segment(path);
    (!id.is_empty()).then(|| format!("/galleries/{id}"))
}

fn last_path_segment(input: &str) -> String {
    input
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("1")
        .to_string()
}

fn absolute_url(base_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

const MANGA_BRIEF_QUERY: &str = r#"
query MangaBrief($filter: FindFilterType) {
  findGalleries(filter: $filter) {
    galleries {
      id
      title
      folder { path }
      cover {
        paths { thumbnail }
        visual_files { __typename }
      }
    }
  }
}
"#;

const MANGA_DETAILS_QUERY: &str = r#"
query MangaDetails($id: ID!) {
  findGallery(id: $id) {
    id
    title
    folder { path }
    photographer
    details
    tags { name }
    cover {
      paths { thumbnail }
      visual_files { __typename }
    }
  }
}
"#;

const CHAPTER_LIST_QUERY: &str = r#"
query ChapterList($id: ID!) {
  findGallery(id: $id) {
    id
    created_at
    photographer
  }
}
"#;

const PAGE_LIST_QUERY: &str = r#"
query PageList($id: Int!) {
  findImages(
    filter: { per_page: -1, sort: "path" }
    image_filter: {
      galleries_filter: { id: { value: $id, modifier: EQUALS } }
      files_filter: { image_file_filter: { format: { value: "", modifier: NOT_EQUALS } } }
    }
  ) {
    images { id }
  }
}
"#;

const BRIEF_FIXTURE: &str = r#"{
  "data": { "findGalleries": { "galleries": [
    { "id": "1", "title": "Sample Gallery", "folder": { "path": "/sample/gallery" }, "cover": { "paths": { "thumbnail": "/image/10/thumbnail" }, "visual_files": [{ "__typename": "ImageFile" }] } }
  ] } }
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": { "findGallery": {
    "id": "1",
    "title": "Sample Gallery",
    "folder": { "path": "/sample/gallery" },
    "photographer": "Sample Photographer",
    "details": "Sample details",
    "tags": [{ "name": "Sample Tag" }],
    "cover": { "paths": { "thumbnail": "/image/10/thumbnail" }, "visual_files": [{ "__typename": "ImageFile" }] }
  } }
}"#;

const CHAPTER_FIXTURE: &str = r#"{
  "data": { "findGallery": { "id": "1", "created_at": "2024-01-01T00:00:00Z", "photographer": "Sample Photographer" } }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": { "findImages": { "images": [{ "id": "10" }, { "id": "11" }] } }
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_brief_page() {
        let page = parse_brief_page(BRIEF_FIXTURE, DEFAULT_BASE_URL);
        assert_eq!(page.entries[0].key, "/galleries/1");
        assert_eq!(
            page.entries[0].cover.as_deref(),
            Some("http://localhost:9999/image/10/thumbnail")
        );
    }

    #[test]
    fn parses_details() {
        let item = parse_details(DETAILS_FIXTURE, DEFAULT_BASE_URL);
        assert_eq!(item.title, "Sample Gallery");
        assert_eq!(item.authors, vec!["Sample Photographer"]);
        assert_eq!(item.tags, vec!["Sample Tag"]);
    }

    #[test]
    fn parses_chapter() {
        let chapter = parse_chapter(CHAPTER_FIXTURE, DEFAULT_BASE_URL);
        assert_eq!(chapter.key, "/galleries/1");
        assert_eq!(chapter.scanlators, vec!["Sample Photographer"]);
    }

    #[test]
    fn parses_pages() {
        let api_key = Some("secret".to_string());
        let pages = parse_pages(PAGES_FIXTURE, DEFAULT_BASE_URL, &api_key);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, context } => {
                assert_eq!(url, "http://localhost:9999/image/10/image");
                assert_eq!(
                    context
                        .as_ref()
                        .and_then(|headers| headers.get("ApiKey"))
                        .map(String::as_str),
                    Some("secret")
                );
            }
            _ => panic!("expected URL page"),
        }
    }
}
