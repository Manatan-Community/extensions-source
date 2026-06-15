use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UpdateStrategy, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Twicomi = Twicomi;
const BASE_URL: &str = "https://twicomi.com";
const API_URL: &str = "https://api.twicomi.com/api/v2";
const PAGE_LIMIT: u64 = 24;
const AUTHOR_PAGE_LIMIT: u64 = 500;
const MAX_AUTHOR_PAGES: u64 = 10;

struct Twicomi;

impl MangaSource for Twicomi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!(
                "{API_URL}/manga/list?order_by=create_time&page_no={page}&page_limit={PAGE_LIMIT}"
            )
        } else {
            format!("{API_URL}/manga/featured/list?page_no={page}&page_limit={PAGE_LIMIT}")
        };
        Ok(parse_manga_response(
            &fetch_json_or_fixture(&target, MANGA_FIXTURE),
            page,
            PAGE_LIMIT,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![catalog_from_key(&normalize_key(query))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let search_type = filters
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("manga");
        let mut target = format!("{API_URL}/{search_type}/list?");
        if !query.is_empty() {
            target.push_str("query=");
            target.push_str(&url::query_escape(query));
            target.push('&');
        }
        let sort_id = if search_type == "author" {
            "authorSort"
        } else {
            "mangaSort"
        };
        append_sort(filters.get(sort_id).and_then(Value::as_str), &mut target);
        target.push_str(&format!("page_no={page}&page_limit=12"));
        let body = fetch_json_or_fixture(
            &target,
            if search_type == "author" {
                AUTHOR_FIXTURE
            } else {
                MANGA_FIXTURE
            },
        );
        Ok(if search_type == "author" {
            parse_author_response(&body, page, 12)
        } else {
            parse_manga_response(&body, page, 12)
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_manga_key);
        Ok(catalog_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_manga_key);
        if key.starts_with("/manga/") {
            return Ok(vec![chapter_from_catalog(&catalog_from_key(&key), 1)]);
        }
        let screen_name = key.trim_matches('/').split('/').nth(1).unwrap_or("sample");
        Ok(fetch_author_chapters(screen_name))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(sample_manga_key);
        Ok(pages_from_key(&key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"listingId": "popular", "page": 1}))?;
        let latest = self.list(serde_json::json!({"listingId": "latest", "page": 1}))?;
        Ok(vec![
            HomeSection {
                id: "featured".to_string(),
                title: "Featured".to_string(),
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
        Ok(manga::request_key(&request, "manga").map(|key| url_from_key(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url_from_key(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key)),
                url: Some(url_from_key(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_author_chapters(screen_name: &str) -> Vec<MangaChapter> {
    let mut items = Vec::new();
    let mut page = 1;
    while page <= MAX_AUTHOR_PAGES {
        let target = format!(
            "{API_URL}/author/manga/list?screen_name={screen_name}&order_by=create_time&order=asc&page_no={page}&page_limit={AUTHOR_PAGE_LIMIT}"
        );
        let body = fetch_json_or_fixture(&target, MANGA_FIXTURE);
        let parsed = parse_manga_items(&body);
        if parsed.is_empty() {
            break;
        }
        items.extend(parsed);
        let total = total_count(&body);
        if page * AUTHOR_PAGE_LIMIT >= total {
            break;
        }
        page += 1;
    }
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let mut chapter = chapter_from_catalog(&item, index + 1);
            chapter.title = item
                .description
                .as_deref()
                .and_then(|text| text.lines().next())
                .map(ToString::to_string)
                .or(chapter.title);
            chapter
        })
        .rev()
        .collect()
}

fn parse_manga_response(body: &str, page: u64, limit: u64) -> Paged<CatalogItem> {
    let entries = parse_manga_items(body);
    Paged {
        has_next_page: page * limit < total_count(body),
        entries,
    }
}

fn parse_author_response(body: &str, page: u64, limit: u64) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/response/author_list")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|wrapper| author_to_catalog(wrapper.get("author")?))
        .collect();
    Paged {
        has_next_page: page * limit < total_count(body),
        entries,
    }
}

fn parse_manga_items(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/response/manga_list")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| manga_item_to_catalog(&item))
        .collect()
}

fn manga_item_to_catalog(item: &Value) -> Option<CatalogItem> {
    let author = item.get("author")?;
    let tweet = item.get("tweet")?;
    let screen_name = string_field(author, "screen_name")?;
    let tweet_id = string_field(tweet, "tweet_id")?;
    let image_urls = tweet
        .get("attach_image_urls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let timestamp =
        parse_twicomi_date(&string_field(tweet, "tweet_create_time").unwrap_or_default())
            .unwrap_or(0);
    let key = format!(
        "/manga/{screen_name}/{tweet_id}#{timestamp},{}",
        image_urls.join(",")
    );
    Some(CatalogItem {
        key: key.clone(),
        title: string_field(tweet, "tweet_text")
            .and_then(|text| text.lines().next().map(ToString::to_string))
            .unwrap_or_else(|| "Tweet".to_string()),
        url: Some(url_from_key(&key)),
        authors: vec![format!(
            "{} (@{})",
            string_field(author, "name").unwrap_or_else(|| screen_name.clone()),
            screen_name
        )],
        description: string_field(tweet, "tweet_text"),
        tags: string_array(tweet, "hash_tags")
            .into_iter()
            .chain(string_array(tweet, "tags"))
            .collect(),
        cover: image_urls.first().cloned(),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn author_to_catalog(author: &Value) -> Option<CatalogItem> {
    let screen_name = string_field(author, "screen_name")?;
    let key = format!("/author/{screen_name}");
    Some(CatalogItem {
        key: key.clone(),
        title: string_field(author, "name").unwrap_or_else(|| screen_name.clone()),
        url: Some(url_from_key(&key)),
        authors: vec![screen_name],
        description: string_field(author, "description"),
        cover: string_field(author, "profile_image"),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn catalog_from_key(key: &str) -> CatalogItem {
    let title = if key.starts_with("/author/") {
        key.trim_matches('/')
            .split('/')
            .nth(1)
            .unwrap_or("Author")
            .to_string()
    } else {
        key.trim_matches('/')
            .split('/')
            .nth(2)
            .unwrap_or("Tweet")
            .to_string()
    };
    CatalogItem {
        key: key.to_string(),
        title,
        url: Some(url_from_key(key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_from_catalog(item: &CatalogItem, number: usize) -> MangaChapter {
    MangaChapter {
        key: item.key.clone(),
        title: Some(item.title.clone()),
        chapter_number: Some(number as f32),
        date_uploaded: item
            .key
            .split('#')
            .nth(1)
            .and_then(|extra| extra.split(',').next())
            .and_then(|value| value.parse::<i64>().ok()),
        url: item.url.clone(),
        ..MangaChapter::default()
    }
}

fn pages_from_key(key: &str) -> Vec<MangaPage> {
    key.split('#')
        .nth(1)
        .unwrap_or_default()
        .split(',')
        .skip(1)
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: None,
            },
            description: Some((index + 1).to_string()),
            ..MangaPage::default()
        })
        .collect()
}

fn url_from_key(key: &str) -> String {
    match key
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default()
    {
        "author" => format!("{BASE_URL}{}/page/1", key.split('#').next().unwrap_or(key)),
        "manga" => format!("{BASE_URL}{}", key.split('#').next().unwrap_or(key)),
        _ => url::join_url(BASE_URL, key),
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        let path = input
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(input);
        if path.starts_with("/author/") || path.starts_with("/manga/") {
            path.to_string()
        } else {
            format!(
                "/manga/unknown/{}",
                url::slug_from_url(path).unwrap_or_else(|| "tweet".into())
            )
        }
    } else {
        input.to_string()
    }
}

fn append_sort(value: Option<&str>, target: &mut String) {
    if let Some((order_by, order)) = value.and_then(|value| value.split_once(':')) {
        target.push_str("order_by=");
        target.push_str(order_by);
        target.push_str("&order=");
        target.push_str(order);
        target.push('&');
    }
}

fn total_count(body: &str) -> u64 {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/response/total_count")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn parse_twicomi_date(value: &str) -> Option<i64> {
    let date = value.split_whitespace().next()?;
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    Some(days_from_civil(parts.next()?, parts.next()?, parts.next()?) * 86_400)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn sample_manga_key() -> String {
    "/manga/sample/1#1704067200,https://cdn.example/page-1.jpg,https://cdn.example/page-2.jpg"
        .to_string()
}

const MANGA_FIXTURE: &str = r#"{
  "status_code": 200,
  "response": {
    "total_count": 1,
    "manga_list": [
      {
        "author": { "screen_name": "sample", "name": "Sample Author" },
        "tweet": {
          "tweet_id": "123",
          "tweet_text": "Sample Tweet\nBody",
          "attach_image_urls": ["https://cdn.example/page-1.jpg", "https://cdn.example/page-2.jpg"],
          "tags": ["sample"],
          "hash_tags": ["tag"],
          "tweet_create_time": "2024-01-01 00:00:00"
        }
      }
    ]
  }
}"#;

const AUTHOR_FIXTURE: &str = r#"{
  "status_code": 200,
  "response": {
    "total_count": 1,
    "author_list": [
      { "author": { "screen_name": "sample", "name": "Sample Author", "description": "Sample bio", "profile_image": "https://cdn.example/profile.jpg" } }
    ]
  }
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manga_and_pages() {
        let page = parse_manga_response(MANGA_FIXTURE, 1, PAGE_LIMIT);
        assert_eq!(page.entries[0].title, "Sample Tweet");
        assert_eq!(page.entries[0].authors, vec!["Sample Author (@sample)"]);
        let pages = pages_from_key(&page.entries[0].key);
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn parses_author() {
        let page = parse_author_response(AUTHOR_FIXTURE, 1, 12);
        assert_eq!(page.entries[0].key, "/author/sample");
        assert_eq!(page.entries[0].title, "Sample Author");
    }

    #[test]
    fn builds_chapter() {
        let item = parse_manga_items(MANGA_FIXTURE).remove(0);
        let chapter = chapter_from_catalog(&item, 1);
        assert_eq!(chapter.date_uploaded, Some(1_704_067_200));
    }
}
