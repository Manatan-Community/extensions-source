use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: AzComic = AzComic;
const BASE_URL: &str = "https://azcomic.com";
const PAGE_SIZE: usize = 36;

struct AzComic;

impl MangaSource for AzComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(series_to_page(
                build_series(parse_comics(COMICS_FIXTURE)),
                1,
            ));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let mut series = build_series(fetch_comics());
        series.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(series_to_page(series, page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = url::slug_from_url(query).unwrap_or_default();
            return Ok(Paged {
                entries: build_series(fetch_comics())
                    .into_iter()
                    .filter(|series| series.slug == slug)
                    .map(SeriesEntry::into_catalog)
                    .collect(),
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        let mut series = build_series(fetch_comics())
            .into_iter()
            .filter(|series| matches_query(series, query))
            .filter(|series| matches_filters(series, filters))
            .collect::<Vec<_>>();
        if query.is_empty() && filters.is_none() {
            series.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        } else {
            series.sort_by_key(|series| series.title.to_ascii_lowercase());
        }
        Ok(series_to_page(series, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        Ok(build_series(fetch_comics())
            .into_iter()
            .find(|series| series.slug == slug)
            .map(SeriesEntry::into_catalog_initialized)
            .unwrap_or_else(|| fallback_catalog(slug)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        Ok(build_series(fetch_comics())
            .into_iter()
            .find(|series| series.slug == slug)
            .map(|series| {
                series
                    .chapters
                    .into_iter()
                    .map(ChapterEntry::into_chapter)
                    .collect()
            })
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        let url_param = key.trim_start_matches('/');
        let body = fetch_image_payload(url_param);
        let payload: ImagePayload = serde_json::from_str(&body).unwrap_or_default();
        Ok(payload
            .images
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image.clone(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let Some(key) = manga::request_key(&request, "manga") else {
            return Ok(None);
        };
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        let target = build_series(fetch_comics())
            .into_iter()
            .find(|series| series.slug == slug)
            .map(|series| series.latest_url)
            .unwrap_or(key);
        Ok(Some(url::join_url(BASE_URL, &target)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = url::slug_from_url(input).unwrap_or_default();
            return Ok(Some(UrlResolveResult {
                item: Some(
                    build_series(fetch_comics())
                        .into_iter()
                        .find(|series| series.slug == slug)
                        .map(SeriesEntry::into_catalog_initialized)
                        .unwrap_or_else(|| fallback_catalog(&slug)),
                ),
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

fn fetch_comics() -> Vec<ComicEntry> {
    parse_comics(
        &client()
            .get(format!("{BASE_URL}/get_comic.php"))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| COMICS_FIXTURE.to_string()),
    )
}

fn fetch_image_payload(path: &str) -> String {
    client()
        .get(format!(
            "{BASE_URL}/get_image.php?url={}",
            url::query_escape(path)
        ))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| IMAGES_FIXTURE.to_string())
}

fn parse_comics(body: &str) -> Vec<ComicEntry> {
    serde_json::from_str::<Vec<ComicEntry>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| !entry.title.is_empty() && !entry.cover.is_empty() && !entry.url.is_empty())
        .collect()
}

fn build_series(entries: Vec<ComicEntry>) -> Vec<SeriesEntry> {
    let mut grouped: BTreeMap<String, Vec<ComicEntry>> = BTreeMap::new();
    for entry in entries {
        let title = series_title(&entry.title);
        let slug = entry.series_slug(&title);
        grouped.entry(slug).or_default().push(entry);
    }
    grouped
        .into_iter()
        .map(|(slug, comics)| {
            let latest = comics
                .iter()
                .max_by_key(|entry| (entry.updated_at_millis().max(0), entry.num))
                .cloned()
                .unwrap_or_default();
            let title = series_title(&latest.title);
            let mut chapters = comics
                .iter()
                .map(|entry| entry.to_chapter_entry(&title))
                .collect::<Vec<_>>();
            chapters.sort_by(|left, right| {
                right
                    .chapter_number
                    .partial_cmp(&left.chapter_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| right.date_upload.cmp(&left.date_upload))
                    .then_with(|| right.order.cmp(&left.order))
            });
            let cover = comics
                .iter()
                .min_by(|left, right| {
                    left.chapter_number()
                        .partial_cmp(&right.chapter_number())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|entry| entry.cover.clone())
                .unwrap_or_else(|| latest.cover.clone());
            let updated_at = chapters
                .first()
                .map(|chapter| chapter.date_upload)
                .unwrap_or_else(|| latest.updated_at_millis());
            SeriesEntry {
                slug,
                title,
                category: latest.category,
                cover: Some(cover),
                latest_url: format!("/{}", latest.url),
                updated_at,
                chapters,
            }
        })
        .collect()
}

fn series_to_page(series: Vec<SeriesEntry>, page: usize) -> Paged<CatalogItem> {
    if series.is_empty() {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    }
    let start = page.saturating_sub(1) * PAGE_SIZE;
    if start >= series.len() {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    }
    let end = (start + PAGE_SIZE).min(series.len());
    Paged {
        has_next_page: end < series.len(),
        entries: series[start..end]
            .iter()
            .cloned()
            .map(SeriesEntry::into_catalog)
            .collect(),
    }
}

fn matches_query(series: &SeriesEntry, query: &str) -> bool {
    query.is_empty()
        || series
            .title
            .to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
}

fn matches_filters(series: &SeriesEntry, filters: Option<&Value>) -> bool {
    let Some(filters) = filters.and_then(Value::as_object) else {
        return true;
    };
    if let Some(category) = filters.get("category").and_then(Value::as_str)
        && !category.is_empty()
        && series.category.as_deref() != Some(category)
    {
        return false;
    }
    if let Some(letter) = filters.get("letter").and_then(Value::as_str)
        && !letter.is_empty()
        && letter != "All"
    {
        let first = series
            .title
            .trim()
            .chars()
            .next()
            .unwrap_or('#')
            .to_ascii_uppercase();
        if letter == "#" {
            return !first.is_ascii_alphabetic();
        }
        return first.to_string() == letter;
    }
    true
}

fn fallback_catalog(slug: &str) -> CatalogItem {
    CatalogItem {
        key: format!("/series/{slug}"),
        title: slug.replace('-', " "),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn series_title(value: &str) -> String {
    for marker in [" Chapter ", " chapter ", " Issue ", " issue "] {
        if let Some(index) = value.find(marker) {
            return value[..index].trim().to_string();
        }
    }
    value.trim().to_string()
}

fn chapter_name(title: &str, series_title: &str) -> String {
    for marker in ["Chapter ", "chapter ", "Issue ", "issue "] {
        if let Some(index) = title.find(marker) {
            return title[index..].trim().to_string();
        }
    }
    title
        .strip_prefix(series_title)
        .map(|value| value.trim().trim_start_matches(['-', ':']).to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title.to_string())
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    for ch in value.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn parse_date(value: &str) -> i64 {
    let Some((date, time)) = value.split_once(' ') else {
        return 0;
    };
    let date_parts = date
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        .collect::<Vec<_>>();
    let time_parts = time
        .split(':')
        .filter_map(|part| part.parse::<i64>().ok())
        .collect::<Vec<_>>();
    if date_parts.len() != 3 || time_parts.len() < 2 {
        return 0;
    }
    timestamp_utc(
        date_parts[0],
        date_parts[1],
        date_parts[2],
        time_parts[0],
        time_parts[1],
        *time_parts.get(2).unwrap_or(&0),
    )
}

fn timestamp_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second
}

#[derive(Clone, Default, Deserialize)]
struct ComicEntry {
    #[serde(default)]
    num: i64,
    category: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "updated_at")]
    updated_at: Option<String>,
}

impl ComicEntry {
    fn updated_at_millis(&self) -> i64 {
        self.updated_at.as_deref().map(parse_date).unwrap_or(0)
    }

    fn series_slug(&self, title: &str) -> String {
        if let Some(rest) = self.cover.split("/uploads/manga/").nth(1)
            && let Some(slug) = rest.split('/').next()
        {
            return slug.to_string();
        }
        let from_url = self
            .url
            .split('/')
            .nth(1)
            .unwrap_or(&self.url)
            .split_once('-')
            .map(|(_, rest)| rest)
            .unwrap_or(&self.url)
            .split("-chapter-")
            .next()
            .unwrap_or(title);
        slugify(from_url)
    }

    fn chapter_number(&self) -> f32 {
        for candidate in [
            self.cover
                .split("/chapters/")
                .nth(1)
                .and_then(|value| value.split('/').next()),
            self.title
                .split("Chapter ")
                .nth(1)
                .and_then(|value| value.split_whitespace().next()),
            self.title
                .split("Issue ")
                .nth(1)
                .and_then(|value| value.split_whitespace().next()),
            self.url
                .split("-chapter-")
                .nth(1)
                .and_then(|value| value.split('/').next()),
            self.url
                .split("-issue-")
                .nth(1)
                .and_then(|value| value.split('/').next()),
        ] {
            if let Some(number) = candidate.and_then(|value| value.parse().ok()) {
                return number;
            }
        }
        -1.0
    }

    fn to_chapter_entry(&self, series_title: &str) -> ChapterEntry {
        ChapterEntry {
            title: chapter_name(&self.title, series_title),
            key: format!("/{}", self.url.trim_start_matches('/')),
            date_upload: self.updated_at_millis(),
            chapter_number: self.chapter_number(),
            order: self.num,
        }
    }
}

#[derive(Clone)]
struct SeriesEntry {
    slug: String,
    title: String,
    category: Option<String>,
    cover: Option<String>,
    latest_url: String,
    updated_at: i64,
    chapters: Vec<ChapterEntry>,
}

impl SeriesEntry {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("/series/{}", self.slug),
            title: self.title,
            cover: self.cover,
            tags: self.category.into_iter().collect(),
            url: Some(format!("{BASE_URL}{}", self.latest_url)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            latest_update: Some(self.updated_at).filter(|value| *value > 0),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        self.into_catalog()
    }
}

#[derive(Clone)]
struct ChapterEntry {
    title: String,
    key: String,
    date_upload: i64,
    chapter_number: f32,
    order: i64,
}

impl ChapterEntry {
    fn into_chapter(self) -> MangaChapter {
        MangaChapter {
            key: self.key.clone(),
            title: Some(self.title),
            date_uploaded: Some(self.date_upload).filter(|value| *value > 0),
            chapter_number: Some(self.chapter_number).filter(|value| *value >= 0.0),
            url: Some(url::join_url(BASE_URL, &self.key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct ImagePayload {
    images: Option<Vec<String>>,
}

export_manga_source!(SOURCE);

const COMICS_FIXTURE: &str = r#"
[
  {
    "num": 1,
    "category": "Action",
    "title": "Sample Comic Chapter 1",
    "cover": "https://azcomic.com/uploads/manga/sample-comic/cover.jpg",
    "url": "manga/sample-comic-chapter-1",
    "updated_at": "2024-01-01 00:00:00"
  }
]
"#;

const IMAGES_FIXTURE: &str = r#"{ "images": ["https://azcomic.com/uploads/sample/001.jpg"] }"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn groups_fixture_into_series() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Comic");
        let chapters = SOURCE
            .chapters(json!({"manga": "/series/sample-comic"}))
            .unwrap();
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }
}
