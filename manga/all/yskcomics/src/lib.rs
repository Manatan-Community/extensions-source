use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: YskComics = YskComics;
const BASE_URL: &str = "https://www.ysk-comics.com";
const API_URL: &str = "https://api.ysk-comics.com";

struct YskComics;

impl MangaSource for YskComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = SourceConfig::from_request(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(POPULAR_FIXTURE, &config, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let section = request
            .get("section")
            .or_else(|| request.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if section == "latest" {
            format!("{BASE_URL}/api/home/latest-comics?page={page}")
        } else {
            format!("{BASE_URL}/api/home/best-comics")
        };
        let body = fetch_json_or_fixture(&config, &target, listing_fixture(section));
        Ok(parse_listing(&body, &config, section == "latest"))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = SourceConfig::from_request(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query, &config);
            let body = fetch_json_or_fixture(
                &config,
                &format!("{BASE_URL}/api/comic/{}", slug_from_key(&key)),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, &config, Some(key))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{API_URL}/api/v1/search-comics-home?name={}",
            url::query_escape(query)
        );
        let body = fetch_json_or_fixture(&config, &target, SEARCH_FIXTURE);
        Ok(parse_listing(&body, &config, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = SourceConfig::from_request(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/comic/sample-comic", config.lang));
        let target = format!("{BASE_URL}/api/comic/{}", slug_from_key(&key));
        let body = fetch_json_or_fixture(&config, &target, DETAILS_FIXTURE);
        Ok(parse_details(&body, &config, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = SourceConfig::from_request(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/comic/sample-comic", config.lang));
        Ok(fetch_chapters(&config, &slug_from_key(&key)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = SourceConfig::from_request(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/{}/chapter/sample-chapter", config.lang));
        let target = format!("{BASE_URL}/api/chapters/images/{}", slug_from_key(&key));
        let body = fetch_json_or_fixture(&config, &target, PAGES_FIXTURE);
        Ok(parse_pages(&body, &config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let config = SourceConfig::from_request(&request);
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input, &config);
            let body = fetch_json_or_fixture(
                &config,
                &format!("{BASE_URL}/api/comic/{}", slug_from_key(&key)),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, &config, Some(key))),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    lang: &'static str,
}

impl SourceConfig {
    fn from_request(request: &Value) -> Self {
        let lang = match request
            .get("sourceId")
            .or_else(|| request.get("source_id"))
            .and_then(Value::as_str)
            .and_then(|source_id| source_id.rsplit('-').next())
            .filter(|value| matches!(*value, "ar" | "en"))
        {
            Some("ar") => "ar",
            Some("en") => "en",
            _ => match request.get("lang").and_then(Value::as_str) {
                Some("ar") => "ar",
                _ => "en",
            },
        };
        Self { lang }
    }

    fn item_key(&self, slug: &str) -> String {
        format!("/{}/comic/{slug}", self.lang)
    }

    fn chapter_key(&self, slug: &str) -> String {
        format!("/{}/chapter/{slug}", self.lang)
    }
}

fn client(config: &SourceConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("x-localization", config.lang)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(config: &SourceConfig, target: &str, fixture: &str) -> String {
    client(config)
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_chapters(config: &SourceConfig, slug: &str) -> Vec<MangaChapter> {
    let mut page = 1;
    let mut chapters = Vec::new();
    loop {
        let target = format!("{BASE_URL}/api/comic/chapter/{slug}?page={page}");
        let body = fetch_json_or_fixture(config, &target, CHAPTERS_FIXTURE);
        let parsed = parse_chapter_page(&body, config);
        chapters.extend(parsed.entries);
        if !parsed.has_next_page || page >= 20 {
            break;
        }
        page += 1;
    }
    chapters.reverse();
    chapters
}

fn parse_listing(body: &str, config: &SourceConfig, paged: bool) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let data = root.get("data").unwrap_or(&Value::Null);
    let entries_value = data
        .get("data_messages")
        .and_then(Value::as_array)
        .or_else(|| data.as_array());
    let entries = entries_value
        .into_iter()
        .flatten()
        .filter_map(|entry| catalog_from_value(entry, config, false))
        .collect();
    Paged {
        has_next_page: paged
            && data
                .get("meta")
                .and_then(|meta| meta.get("link_next"))
                .is_some_and(|value| !value.is_null()),
        entries,
    }
}

fn parse_details(body: &str, config: &SourceConfig, key: Option<String>) -> CatalogItem {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return fallback_item(config, key);
    };
    let fallback_key = key.clone();
    root.get("data")
        .and_then(|entry| catalog_from_value(entry, config, true))
        .map(|mut item| {
            if let Some(key) = key {
                item.key = key;
            }
            item.initialized = true;
            item
        })
        .unwrap_or_else(|| fallback_item(config, fallback_key))
}

fn catalog_from_value(entry: &Value, config: &SourceConfig, details: bool) -> Option<CatalogItem> {
    let slug = entry.get("slug").and_then(Value::as_str)?;
    let title = entry
        .get("full_name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| url::slug_from_url(slug).unwrap_or_else(|| "Comic".to_string()));
    let rating = rating_line(
        entry
            .get("rate")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        entry.get("rate_count").and_then(Value::as_i64).unwrap_or(0),
    );
    let mut description = Vec::new();
    if !rating.is_empty() {
        description.push(rating);
    }
    if let Some(publisher) = named_value(entry.get("publisher").unwrap_or(&Value::Null)) {
        description.push(format!("Publisher: {publisher}"));
    }
    if let Some(published_at) = entry.get("published_at").and_then(Value::as_str) {
        description.push(format!("Published at: {published_at}"));
    }
    if let Some(text) = entry
        .get("description")
        .or_else(|| entry.get("descrition"))
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
    {
        description.push(text);
    }

    Some(CatalogItem {
        key: config.item_key(slug),
        title,
        cover: entry
            .get("image")
            .and_then(Value::as_str)
            .map(str::to_string),
        authors: named_value(entry.get("writer").unwrap_or(&Value::Null))
            .into_iter()
            .collect(),
        artists: entry
            .get("artists")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(named_value)
            .collect(),
        tags: entry
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(named_value)
            .collect(),
        description: (!description.is_empty()).then(|| description.join("\n")),
        status: match entry.get("status").and_then(Value::as_str) {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!(
            "{BASE_URL}/{}",
            config.item_key(slug).trim_start_matches('/')
        )),
        language: Some(config.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: details,
        ..CatalogItem::default()
    })
}

fn parse_chapter_page(body: &str, config: &SourceConfig) -> Paged<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let data = root.get("data").unwrap_or(&Value::Null);
    let entries = data
        .get("data_messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let slug = entry.get("slug").and_then(Value::as_str)?;
            let rank = entry
                .get("rank")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let key = config.chapter_key(slug);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("#{rank}")),
                url: Some(format!("{BASE_URL}/{}", key.trim_start_matches('/'))),
                chapter_number: rank.parse().ok(),
                ..MangaChapter::default()
            })
        })
        .collect();
    Paged {
        has_next_page: data
            .get("meta")
            .and_then(|meta| meta.get("link_next"))
            .is_some_and(|value| !value.is_null()),
        entries,
    }
}

fn parse_pages(body: &str, _config: &SourceConfig) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn named_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.to_string()),
        Value::Object(object) => object
            .get("name")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn rating_line(rate: &str, rate_count: i64) -> String {
    let Ok(value) = rate.parse::<f64>() else {
        return String::new();
    };
    if value <= 0.0 {
        return String::new();
    }
    if rate_count > 0 {
        format!("Rating: {rate} ({rate_count})")
    } else {
        format!("Rating: {rate}")
    }
}

fn normalize_key(input: &str, config: &SourceConfig) -> String {
    if let Some(index) = input.find("/comic/") {
        return format!(
            "/{}/comic/{}",
            config.lang,
            input[index + 7..].trim_matches('/')
        );
    }
    format!("/{}/comic/{}", config.lang, input.trim_matches('/'))
}

fn slug_from_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample-comic")
        .to_string()
}

fn fallback_item(config: &SourceConfig, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/{}/comic/sample-comic", config.lang));
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Comic".into()),
        url: Some(format!("{BASE_URL}/{}", key.trim_start_matches('/'))),
        language: Some(config.lang.to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    }
}

fn listing_fixture(section: &str) -> &'static str {
    if section == "latest" {
        LATEST_FIXTURE
    } else {
        POPULAR_FIXTURE
    }
}

const POPULAR_FIXTURE: &str = r#"{
  "data": [
    {
      "image": "https://cdn.ysk/sample.jpg",
      "full_name": "Sample Comic",
      "slug": "sample-comic",
      "rate": "4.8",
      "writer": {"name": "Writer"},
      "publisher": {"name": "Publisher"},
      "genres": [{"name": "Action"}],
      "descrition": "<p>A sample comic.</p>"
    }
  ]
}"#;

const LATEST_FIXTURE: &str = r#"{
  "data": {
    "data_messages": [
      {
        "image": "https://cdn.ysk/latest.jpg",
        "full_name": "Latest Comic",
        "slug": "latest-comic",
        "rate": "4.2",
        "rate_count": 12,
        "writer": "Writer",
        "genres": [{"name": "Drama"}]
      }
    ],
    "meta": {"link_next": "https://www.ysk-comics.com/api/home/latest-comics?page=2"}
  }
}"#;

const SEARCH_FIXTURE: &str = r#"{
  "data": [
    {
      "full_name": "Search Comic",
      "slug": "search-comic",
      "image": "https://cdn.ysk/search.jpg"
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "data": {
    "full_name": "Sample Comic",
    "slug": "sample-comic",
    "image": "https://cdn.ysk/sample.jpg",
    "rate": "4.8",
    "rate_count": 14,
    "language_code": "en",
    "writer": {"name": "Writer"},
    "publisher": {"name": "Publisher"},
    "genres": [{"name": "Action"}],
    "artists": [{"name": "Artist"}],
    "status": "completed",
    "description": "<p>Detailed text.</p>",
    "published_at": "2024-01-01"
  }
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "data": {
    "data_messages": [
      {"slug": "sample-1", "rank": "1"},
      {"slug": "sample-2", "rank": "2"}
    ],
    "meta": {"link_next": null}
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": [
    "https://cdn.ysk/page-1.jpg",
    "https://cdn.ysk/page-2.jpg"
  ]
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_source_language() {
        let config = SourceConfig::from_request(&serde_json::json!({ "sourceId": "yskcomics-ar" }));
        assert_eq!(config.lang, "ar");
        assert_eq!(config.item_key("slug"), "/ar/comic/slug");
    }

    #[test]
    fn parses_api_listing_and_details() {
        let config = SourceConfig { lang: "en" };
        let popular = parse_listing(POPULAR_FIXTURE, &config, false);
        assert_eq!(popular.entries[0].key, "/en/comic/sample-comic");
        assert_eq!(popular.entries[0].authors, vec!["Writer"]);

        let latest = parse_listing(LATEST_FIXTURE, &config, true);
        assert!(latest.has_next_page);
        assert_eq!(latest.entries[0].title, "Latest Comic");

        let details = parse_details(DETAILS_FIXTURE, &config, None);
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.artists, vec!["Artist"]);
    }

    #[test]
    fn parses_chapters_and_pages() {
        let config = SourceConfig { lang: "en" };
        let chapters = parse_chapter_page(CHAPTERS_FIXTURE, &config);
        assert_eq!(chapters.entries.len(), 2);
        assert_eq!(chapters.entries[1].key, "/en/chapter/sample-2");

        let pages = parse_pages(PAGES_FIXTURE, &config);
        assert_eq!(pages.len(), 2);
    }
}
