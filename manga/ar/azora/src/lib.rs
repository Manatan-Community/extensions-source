use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Azora = Azora;
const BASE_URL: &str = "https://azoramoon.com";
const API_URL: &str = "https://api.azoramoon.com";

struct Azora;

impl MangaSource for Azora {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE));
        }
        let filters = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            serde_json::json!({"orderBy":"totalViews","orderDirection":"desc"})
        } else {
            serde_json::json!({"orderBy":"lastChapterAddedAt","orderDirection":"desc"})
        };
        let target = query_url(
            request.get("page").and_then(Value::as_u64).unwrap_or(1),
            "",
            &filters,
        );
        Ok(parse_search(&fetch_json_or_fixture(
            &target,
            SEARCH_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            let body = fetch_json_or_fixture(
                &format!("{API_URL}/api/post?postSlug={slug}"),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = query_url(
            request.get("page").and_then(Value::as_u64).unwrap_or(1),
            query,
            filters,
        );
        Ok(parse_search(&fetch_json_or_fixture(
            &target,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = key.split('#').next().unwrap_or(&key);
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/api/post?postSlug={slug}"),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = key.split('#').next().unwrap_or(&key);
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/api/post?postSlug={slug}"),
            DETAILS_FIXTURE,
        );
        let show_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("showLockedChapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(&body, show_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1#11".to_string());
        let id = key.rsplit('#').next().unwrap_or("11");
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/api/chapter?chapterId={id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json_or_fixture(
                    &format!("{API_URL}/api/post?postSlug={slug}"),
                    DETAILS_FIXTURE,
                ))),
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn query_url(page: u64, query: &str, filters: &Value) -> String {
    let mut target = format!(
        "{API_URL}/api/query?page={page}&perPage=18&searchTerm={}",
        url::query_escape(query.trim())
    );
    for key in ["seriesStatus", "seriesType", "orderBy", "orderDirection"] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            target.push('&');
            target.push_str(key);
            target.push('=');
            target.push_str(&url::query_escape(value));
        }
    }
    target
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let entries = root
        .get("posts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("isNovel").and_then(Value::as_bool) != Some(true))
        .filter_map(catalog_from_value)
        .collect::<Vec<_>>();
    let total = root
        .get("totalCount")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: total > entries.len() as u64,
        entries,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return fallback_item();
    };
    root.get("post")
        .and_then(catalog_from_value)
        .unwrap_or_else(fallback_item)
}

fn catalog_from_value(entry: &Value) -> Option<CatalogItem> {
    let slug = entry.get("slug").and_then(Value::as_str)?;
    let id = entry.get("id").and_then(Value::as_i64).unwrap_or_default();
    let mut tags = Vec::new();
    if let Some(series_type) = entry.get("seriesType").and_then(Value::as_str) {
        tags.push(series_type.to_ascii_lowercase());
    }
    tags.extend(
        entry
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| {
                genre
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
    );
    Some(CatalogItem {
        key: format!("{slug}#{id}"),
        title: entry
            .get("postTitle")
            .and_then(Value::as_str)
            .unwrap_or("Manga")
            .to_string(),
        cover: entry
            .get("featuredImage")
            .and_then(Value::as_str)
            .map(str::to_string),
        authors: string_field(entry, "author").into_iter().collect(),
        artists: string_field(entry, "artist").into_iter().collect(),
        description: description(entry),
        tags,
        status: match entry.get("seriesStatus").and_then(Value::as_str) {
            Some("ONGOING") | Some("COMING_SOON") => ItemStatus::Ongoing,
            Some("COMPLETED") => ItemStatus::Completed,
            Some("CANCELLED") | Some("DROPPED") => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_chapters(body: &str, show_locked: bool) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let post = root.get("post").unwrap_or(&Value::Null);
    let manga_slug = post.get("slug").and_then(Value::as_str).unwrap_or("sample");
    post.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("chapterStatus").and_then(Value::as_str) == Some("PUBLIC"))
        .filter(|entry| {
            entry.get("isAccessible").and_then(Value::as_bool) == Some(true)
                || (show_locked
                    && (entry.get("isLocked").and_then(Value::as_bool) == Some(true)
                        || entry.get("isTimeLocked").and_then(Value::as_bool) == Some(true)))
        })
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_i64)?;
            let slug = entry.get("slug").and_then(Value::as_str)?;
            let number = entry
                .get("number")
                .map(number_text)
                .unwrap_or_else(|| "?".to_string());
            let title = string_field(entry, "title");
            let prefix = if entry.get("isAccessible").and_then(Value::as_bool) == Some(true) {
                ""
            } else {
                "Locked "
            };
            Some(MangaChapter {
                key: format!("/series/{manga_slug}/{slug}#{id}"),
                title: Some(format!(
                    "{prefix}Chapter {number}{}",
                    title.map(|t| format!(" - {t}")).unwrap_or_default()
                )),
                date_uploaded: entry
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(manatan_shared::dates::parse_fixture_date),
                chapter_number: number.parse().ok(),
                url: Some(format!("{BASE_URL}/series/{manga_slug}/{slug}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let chapter = root.get("chapter").unwrap_or(&Value::Null);
    if [
        "isPermanentlyLocked",
        "isLockedByCoins",
        "isShortLinkLocked",
    ]
    .into_iter()
    .any(|key| chapter.get(key).and_then(Value::as_bool) == Some(true))
    {
        return Vec::new();
    }
    let mut images = chapter
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some((
                entry
                    .get("order")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX),
                entry
                    .get("url")
                    .and_then(Value::as_str)?
                    .replace(' ', "%20"),
            ))
        })
        .collect::<Vec<_>>();
    images.sort_by_key(|(order, _)| *order);
    images
        .into_iter()
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
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

fn string_field(entry: &Value, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn description(entry: &Value) -> Option<String> {
    let text = string_field(entry, "postContent").map(|value| html::strip_tags(&value));
    let alt = string_field(entry, "alternativeTitles");
    match (text, alt) {
        (Some(text), Some(alt)) => Some(format!("{text}\n\nAlternative Names: {alt}")),
        (Some(text), None) => Some(text),
        (None, Some(alt)) => Some(format!("Alternative Names: {alt}")),
        _ => None,
    }
}

fn number_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_f64().map(|number| number.to_string()))
        .unwrap_or_else(|| "?".to_string())
}

fn fallback_item() -> CatalogItem {
    CatalogItem {
        key: "sample#1".to_string(),
        title: "Manga".to_string(),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    }
}

const SEARCH_FIXTURE: &str = r#"{
  "posts": [{"id":1,"slug":"sample","postTitle":"Sample Manga","postContent":"<p>Summary</p>","featuredImage":"https://img/sample.jpg","author":"Writer","artist":"Artist","seriesType":"MANGA","seriesStatus":"ONGOING","genres":[{"id":1,"name":"Action"}]}],
  "totalCount": 1
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "post": {
    "id":1,"slug":"sample","postTitle":"Sample Manga","postContent":"<p>Summary</p>","featuredImage":"https://img/sample.jpg","author":"Writer","artist":"Artist","seriesType":"MANGA","seriesStatus":"COMPLETED","genres":[{"id":1,"name":"Action"}],
    "chapters":[{"id":11,"slug":"chapter-1","number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","chapterStatus":"PUBLIC","isAccessible":true,"isLocked":false,"isTimeLocked":false}]
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "chapter": {"id":11,"images":[{"url":"https://img/page 2.jpg","order":2},{"url":"https://img/page1.jpg","order":1}],"isPermanentlyLocked":false,"isLockedByCoins":false,"isShortLinkLocked":false}
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iken_source() {
        let listing = parse_search(SEARCH_FIXTURE);
        assert_eq!(listing.entries[0].key, "sample#1");

        let details = parse_details(DETAILS_FIXTURE);
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(DETAILS_FIXTURE, false);
        assert_eq!(chapters[0].key, "/series/sample/chapter-1#11");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
        let PageContent::Url { url, .. } = &pages[1].content else {
            panic!("expected url page");
        };
        assert!(url.contains("%20"));
    }
}
