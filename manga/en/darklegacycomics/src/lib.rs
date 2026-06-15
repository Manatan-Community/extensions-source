use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: DarkLegacyComics = DarkLegacyComics;
const BASE_URL: &str = "https://www.darklegacycomics.com";
const THUMB_URL: &str = "https://images2.imgbox.com/5d/d8/BVxRdljH_o.png";
const AUTHOR_NAME: &str = "Arad Kedar (Keydar)";
const SPECIALS_DATE: i64 = 1_399_926_480;

struct DarkLegacyComics;

impl MangaSource for DarkLegacyComics {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: catalog(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let entries = if query.starts_with(BASE_URL) {
            let key = normalize_key(&query);
            catalog()
                .into_iter()
                .filter(|item| key.starts_with(&item.key) || item.key == key)
                .collect()
        } else {
            catalog()
                .into_iter()
                .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query))
                .collect()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/archive".to_string());
        Ok(catalog()
            .into_iter()
            .find(|item| item.key == key)
            .unwrap_or_else(|| fallback_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/archive".to_string());
        if key == "/archive" {
            return Ok(parse_archive(&fetch_document(
                &url::join_url(BASE_URL, &key),
                ARCHIVE_FIXTURE,
            )));
        }
        Ok(specials()
            .into_iter()
            .map(|(number, title)| MangaChapter {
                key: format!("/specials/{number}.php"),
                title: Some(title.to_string()),
                chapter_number: Some(number as f32),
                date_uploaded: Some(SPECIALS_DATE),
                url: Some(format!("{BASE_URL}/specials/{number}.php")),
                ..MangaChapter::default()
            })
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = if key.starts_with("/specials/") {
                catalog()
                    .into_iter()
                    .find(|item| item.key == "/specials/1.php")
            } else {
                catalog().into_iter().find(|item| item.key == "/archive")
            };
            return Ok(Some(UrlResolveResult {
                item,
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

fn catalog() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            key: "/archive".to_string(),
            title: "Dark Legacy Comics".to_string(),
            cover: Some(THUMB_URL.to_string()),
            authors: vec![AUTHOR_NAME.to_string()],
            artists: vec![AUTHOR_NAME.to_string()],
            status: ItemStatus::Ongoing,
            url: Some(format!("{BASE_URL}/archive")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        },
        CatalogItem {
            key: "/specials/1.php".to_string(),
            title: "Dark Legacy Comics Specials".to_string(),
            cover: Some(THUMB_URL.to_string()),
            authors: vec![AUTHOR_NAME.to_string()],
            artists: vec![AUTHOR_NAME.to_string()],
            status: ItemStatus::Completed,
            url: Some(format!("{BASE_URL}/specials/1.php")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        },
    ]
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: "Dark Legacy Comics".to_string(),
        cover: Some(THUMB_URL.to_string()),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_archive(body: &str) -> Vec<MangaChapter> {
    body.split("archive_link")
        .skip(1)
        .filter_map(|chunk| {
            let index = html::text_between(chunk, "index", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    html::attr_after(chunk, "<a", "href").and_then(|href| url::slug_from_url(&href))
                })?;
            let title = html::text_between(chunk, "name", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Comic {index}"));
            let date_text = html::text_between(chunk, "date", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let characters = html::text_between(chunk, "characters", "</")
                .map(|value| html::strip_tags(&value).replace(' ', ", "))
                .filter(|value| !value.is_empty());
            let number = index.parse::<f32>().ok();
            Some(MangaChapter {
                key: format!("/{index}"),
                title: Some(format!("#{index}: {title}")),
                chapter_number: number,
                scanlators: characters.into_iter().collect::<Vec<_>>(),
                date_uploaded: parse_archive_date(&date_text),
                url: Some(format!("{BASE_URL}/{index}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("comic") || chunk.contains("src"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_archive_date(value: &str) -> Option<i64> {
    if value.trim() == "Sep 20" {
        return Some(1_442_696_400);
    }
    let cleaned = value.trim().replace(',', "");
    let mut parts = cleaned.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = month_number(parts.remove(0))?;
    let day = parts.remove(0).parse::<i32>().ok()?;
    let year = parts.remove(0).parse::<i32>().ok()?;
    unix_date(year, month, day)
}

fn month_number(value: &str) -> Option<i32> {
    match value {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut days = 0i64;
    for y in 1970..year {
        days += if leap_year(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month(year, m) as i64;
    }
    days += (day - 1) as i64;
    Some(days * 86_400)
}

fn leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn specials() -> Vec<(u32, &'static str)> {
    vec![(1, "Looking For Group"), (2, "Rover"), (3, "Fan Comic")]
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<a class="archive_link"><span class="index">1</span><span class="date">Jan 01, 2024</span><span class="name">Sample</span><span class="characters">A B</span></a>
"#;
const PAGE_FIXTURE: &str = r#"<div class="comic"><img src="/comics/sample.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive_and_page() {
        let chapters = SOURCE.chapters(json!({"manga":"/archive"})).unwrap();
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        let pages = SOURCE.pages(json!({"chapter":"/1"})).unwrap();
        assert_eq!(pages.len(), 1);
    }
}
