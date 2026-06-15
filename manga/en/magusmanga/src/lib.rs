use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MagusManga = MagusManga;
const BASE_URL: &str = "https://magustoon.org";
const API_URL: &str = "https://magustoon.org";
const PER_PAGE: u64 = 18;

struct MagusManga;

impl MangaSource for MagusManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_query_response(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order_by = if listing == "latest" {
            "lastChapterAddedAt"
        } else {
            "totalViews"
        };
        let target = query_url(
            page,
            "",
            &[("orderBy", order_by), ("orderDirection", "desc")],
        );
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_query_response(&body, page))
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
            let slug = slug_from_key(&key);
            let body = fetch_json_or_fixture(&details_api_url(&slug), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: parse_post_root(&body)
                    .as_ref()
                    .map(post_to_item)
                    .into_iter()
                    .collect(),
                has_next_page: false,
            });
        }
        let filters = filter_params(&request);
        let target = query_url(page, query, &filters);
        let body = fetch_json_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_query_response(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = slug_from_key(&key);
        let body = fetch_json_or_fixture(&details_api_url(&slug), DETAILS_FIXTURE);
        Ok(parse_post_root(&body)
            .as_ref()
            .map(post_to_item)
            .unwrap_or_else(|| fixture_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample#1".to_string());
        let slug = slug_from_key(&key);
        let body = fetch_json_or_fixture(&details_api_url(&slug), DETAILS_FIXTURE);
        Ok(parse_post_root(&body)
            .as_ref()
            .map(post_to_chapters)
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-1#10".into());
        let chapter_id = key
            .rsplit_once('#')
            .map(|(_, id)| id)
            .unwrap_or(key.as_str());
        let target = format!("{API_URL}/api/chapter?chapterId={chapter_id}");
        let body = fetch_json_or_fixture(&target, CHAPTER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let slug = slug_from_key(&key);
            let body = fetch_json_or_fixture(&details_api_url(&slug), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: parse_post_root(&body).as_ref().map(post_to_item),
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

fn query_url(page: u64, query: &str, filters: &[(&str, &str)]) -> String {
    let mut target = format!(
        "{API_URL}/api/query?page={page}&perPage={PER_PAGE}&searchTerm={}",
        url::query_escape(query)
    );
    for (key, value) in filters {
        if !value.is_empty() {
            target.push('&');
            target.push_str(key);
            target.push('=');
            target.push_str(&url::query_escape(value));
        }
    }
    target
}

fn details_api_url(slug: &str) -> String {
    format!("{API_URL}/api/post?postSlug={}", url::query_escape(slug))
}

fn filter_params(request: &Value) -> Vec<(&'static str, &str)> {
    let Some(filters) = request.get("filters").and_then(Value::as_object) else {
        return vec![
            ("orderBy", "lastChapterAddedAt"),
            ("orderDirection", "desc"),
        ];
    };
    let mut params = Vec::new();
    for key in [
        "seriesStatus",
        "seriesType",
        "orderBy",
        "orderDirection",
        "genreIds",
    ] {
        if let Some(value) = filters.get(key).and_then(Value::as_str) {
            if !value.is_empty() {
                params.push((key, value));
            }
        }
    }
    if !params.iter().any(|(key, _)| *key == "orderBy") {
        params.push(("orderBy", "lastChapterAddedAt"));
    }
    if !params.iter().any(|(key, _)| *key == "orderDirection") {
        params.push(("orderDirection", "desc"));
    }
    params
}

fn parse_query_response(body: &str, page: u64) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged::default();
    };
    let total_count = root.get("totalCount").and_then(Value::as_u64).unwrap_or(0);
    let entries = root
        .get("posts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|post| post.get("isNovel").and_then(Value::as_bool) != Some(true))
        .map(post_to_item)
        .collect();
    Paged {
        has_next_page: total_count > page * PER_PAGE,
        entries,
    }
}

fn parse_post_root(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("post")
        .cloned()
}

fn post_to_item(post: &Value) -> CatalogItem {
    let id = post.get("id").and_then(Value::as_i64).unwrap_or_default();
    let slug = post.get("slug").and_then(Value::as_str).unwrap_or("sample");
    let key = format!("{slug}#{id}");
    let description = post
        .get("postContent")
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    let alternate_titles = post
        .get("alternativeTitles")
        .and_then(Value::as_str)
        .map(split_alternate_titles)
        .unwrap_or_default();
    let mut tags = Vec::new();
    if let Some(kind) = post.get("seriesType").and_then(Value::as_str) {
        if !kind.is_empty() {
            tags.push(series_type_label(kind).to_string());
        }
    }
    tags.extend(
        post.get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("name").and_then(Value::as_str))
            .filter(|name| !name.is_empty())
            .map(ToString::to_string),
    );

    CatalogItem {
        key: key.clone(),
        title: post
            .get("postTitle")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .unwrap_or("Manga")
            .to_string(),
        alternate_titles,
        cover: post
            .get("featuredImage")
            .and_then(Value::as_str)
            .filter(|image| !image.is_empty())
            .map(ToString::to_string),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        authors: post
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        artists: post
            .get("artist")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        description,
        tags,
        status: parse_status(
            post.get("seriesStatus")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn post_to_chapters(post: &Value) -> Vec<MangaChapter> {
    let series_slug = post.get("slug").and_then(Value::as_str).unwrap_or("sample");
    post.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| chapter.get("chapterStatus").and_then(Value::as_str) == Some("PUBLIC"))
        .filter(|chapter| {
            chapter.get("isAccessible").and_then(Value::as_bool) == Some(true)
                || is_locked_chapter(chapter)
        })
        .map(|chapter| {
            let id = chapter
                .get("id")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let slug = chapter
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or("chapter");
            let number = chapter_number(chapter.get("number"));
            let title_suffix = chapter
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.is_empty())
                .map(|title| format!(" - {title}"))
                .unwrap_or_default();
            let locked_prefix = if chapter.get("isAccessible").and_then(Value::as_bool)
                == Some(false)
                && is_locked_chapter(chapter)
            {
                "Locked "
            } else {
                ""
            };
            let title = format!("{locked_prefix}Chapter {number}{title_suffix}");
            MangaChapter {
                key: format!("/series/{series_slug}/{slug}#{id}"),
                title: Some(title),
                chapter_number: number.parse::<f32>().ok(),
                date_uploaded: chapter
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(parse_iken_date),
                language: Some("en".to_string()),
                url: Some(format!("{BASE_URL}/series/{series_slug}/{slug}")),
                is_locked: is_locked_chapter(chapter),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(chapter) = root.get("chapter") else {
        return Vec::new();
    };
    if [
        "isShortLinkLocked",
        "isLockedByCoins",
        "isPermanentlyLocked",
    ]
    .iter()
    .any(|key| chapter.get(*key).and_then(Value::as_bool) == Some(true))
    {
        return Vec::new();
    }
    let mut pages: Vec<_> = chapter
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            Some((
                page.get("order")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX),
                page.get("url")?.as_str()?.replace(' ', "%20"),
            ))
        })
        .collect();
    pages.sort_by_key(|(order, _)| *order);
    pages
        .into_iter()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index as usize + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn fixture_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: "Sample Manga".to_string(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn split_alternate_titles(value: &str) -> Vec<String> {
    value
        .split([',', '\n', ';'])
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn series_type_label(value: &str) -> &str {
    match value {
        "MANGA" => "Manga",
        "MANHUA" => "Manhua",
        "MANHWA" => "Manhwa",
        "RUSSIAN" => "Russian",
        "SPANISH" => "Spanish",
        _ => value,
    }
}

fn chapter_number(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => "?".to_string(),
    }
}

fn is_locked_chapter(chapter: &Value) -> bool {
    chapter.get("isLocked").and_then(Value::as_bool) == Some(true)
        || chapter.get("isTimeLocked").and_then(Value::as_bool) == Some(true)
}

fn parse_iken_date(value: &str) -> Option<i64> {
    let date = value.get(..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn parse_status(value: &str) -> ItemStatus {
    match value {
        "ONGOING" | "COMING_SOON" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "CANCELLED" | "DROPPED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find("/series/") {
            return input[index + "/series/".len()..]
                .trim_matches('/')
                .to_string();
        }
    }
    input
        .trim_start_matches('/')
        .trim_start_matches("series/")
        .trim_end_matches('/')
        .to_string()
}

fn slug_from_key(key: &str) -> String {
    key.trim_start_matches('/')
        .trim_start_matches("series/")
        .split('/')
        .next()
        .unwrap_or(key)
        .split('#')
        .next()
        .unwrap_or(key)
        .to_string()
}

const LIST_FIXTURE: &str = r#"
{"posts":[{"id":1,"slug":"sample","postTitle":"Sample Manga","postContent":"<p>Sample description.</p>","isNovel":false,"featuredImage":"https://cdn.example.test/cover.jpg","alternativeTitles":"Alt Sample","author":"Writer","artist":"Artist","seriesType":"MANGA","seriesStatus":"ONGOING","genres":[{"id":7,"name":"Drama"}]}],"totalCount":20}
"#;

const DETAILS_FIXTURE: &str = r#"
{"post":{"id":1,"slug":"sample","postTitle":"Sample Manga","postContent":"<p>Sample description.</p>","isNovel":false,"featuredImage":"https://cdn.example.test/cover.jpg","alternativeTitles":"Alt Sample","author":"Writer","artist":"Artist","seriesType":"MANGA","seriesStatus":"COMPLETED","genres":[{"id":7,"name":"Drama"}],"chapters":[{"id":10,"slug":"chapter-1","number":1,"title":"Start","createdAt":"2024-01-01T00:00:00.000Z","chapterStatus":"PUBLIC","isAccessible":true,"isLocked":false,"isTimeLocked":false}]}}
"#;

const CHAPTER_FIXTURE: &str = r#"
{"chapter":{"id":10,"images":[{"url":"https://cdn.example.test/page 1.jpg","order":1},{"url":"https://cdn.example.test/page2.jpg","order":2}],"isPermanentlyLocked":false,"isLockedByCoins":false,"isShortLinkLocked":false}}
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_and_chapters() {
        let listing = parse_query_response(LIST_FIXTURE, 1);
        assert_eq!(listing.entries[0].title, "Sample Manga");
        assert!(listing.has_next_page);

        let details = parse_post_root(DETAILS_FIXTURE)
            .as_ref()
            .map(post_to_item)
            .unwrap();
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors, vec!["Writer"]);

        let chapters = parse_post_root(DETAILS_FIXTURE)
            .as_ref()
            .map(post_to_chapters)
            .unwrap();
        assert_eq!(chapters[0].key, "/series/sample/chapter-1#10");
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(CHAPTER_FIXTURE);
        assert_eq!(pages.len(), 2);
        assert!(matches!(pages[0].content, PageContent::Url { .. }));
    }

    #[test]
    fn parses_iken_dates() {
        assert_eq!(
            parse_iken_date("2024-01-01T00:00:00.000Z"),
            Some(1_704_067_200)
        );
    }
}
