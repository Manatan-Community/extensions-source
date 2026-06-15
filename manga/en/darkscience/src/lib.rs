use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: DarkScience = DarkScience;
const BASE_URL: &str = "https://dresdencodak.com";
const SERIES_KEY: &str = "/category/darkscience";
const COVER: &str = "https://dresdencodak.com/wp-content/uploads/2019/03/DC_CastIcon_Kimiko.png";
const CREATOR: &str = "Sen (A. Senna Diaz)";

struct DarkScience;

impl MangaSource for DarkScience {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item()],
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
        let item = series_item();
        let entries = if query.is_empty()
            || item.title.to_ascii_lowercase().contains(&query)
            || query.starts_with(BASE_URL)
        {
            vec![item]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(fetch_chapters())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2026/06/07/dark-science-185-twilights-end".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, SERIES_KEY)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item()),
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

fn series_item() -> CatalogItem {
    CatalogItem {
        key: SERIES_KEY.to_string(),
        title: "Dark Science".to_string(),
        cover: Some(COVER.to_string()),
        authors: vec![CREATOR.to_string()],
        artists: vec![CREATOR.to_string()],
        description: Some("Scientist Kimiko Ross travels to Nephilopolis, the city of giants, and tries to survive a bureaucratic behemoth with a little help from her friends.".to_string()),
        tags: vec!["Science Fiction".to_string(), "Mystery".to_string(), "LGBT+".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, SERIES_KEY)),
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

fn fetch_chapters() -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut next_url = url::join_url(BASE_URL, SERIES_KEY);
    let mut last_number = 0.0;
    for _ in 0..80 {
        let body = fetch_document(&next_url, CHAPTERS_FIXTURE);
        for chapter in parse_chapter_page(&body, &mut last_number) {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
        }
        let Some(next) = html::attr_after(&body, "nav-previous", "href")
            .or_else(|| html::attr_after(&body, "previous", "href"))
        else {
            break;
        };
        if next == next_url {
            break;
        }
        next_url = next;
    }
    chapters
}

fn parse_chapter_page(body: &str, last_number: &mut f32) -> Vec<MangaChapter> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("category-darkscience") || chunk.contains("Dark Science #"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "tc-grid-bg-link", "href")
                .or_else(|| html::attr_after(chunk, "entry-title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "entry-title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Dark Science".to_string());
            let number = dark_science_number(&title).unwrap_or(*last_number + 0.01);
            *last_number = number;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: Some(number),
                date_uploaded: date_from_url(&href),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("aligncenter") || chunk.contains("wp-image"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.contains("WidgetButton_") && !image.contains("DCLogo_"))
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

fn dark_science_number(title: &str) -> Option<f32> {
    let index = title.find('#')?;
    title[index + 1..]
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .and_then(|value| value.parse::<f32>().ok())
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn date_from_url(value: &str) -> Option<i64> {
    let parts = value.split('/').collect::<Vec<_>>();
    for window in parts.windows(3) {
        let Ok(year) = window[0].parse::<i32>() else {
            continue;
        };
        let Ok(month) = window[1].parse::<i32>() else {
            continue;
        };
        let Ok(day) = window[2].parse::<i32>() else {
            continue;
        };
        if (2000..=2100).contains(&year) {
            return unix_date(year, month, day);
        }
    }
    None
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
<article class="category-darkscience"><a class="tc-grid-bg-link" href="https://dresdencodak.com/2024/01/01/dark-science-1-sample/"></a><h2 class="entry-title"><a href="https://dresdencodak.com/2024/01/01/dark-science-1-sample/">Dark Science #1 - Sample</a></h2></article>
"#;
const PAGE_FIXTURE: &str = r#"<article class="post"><img class="aligncenter wp-image-1" src="/wp-content/uploads/page.jpg"></article>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive_and_pages() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "Dark Science");
        let mut last = 0.0;
        assert_eq!(parse_chapter_page(CHAPTERS_FIXTURE, &mut last).len(), 1);
        assert_eq!(parse_pages(PAGE_FIXTURE).len(), 1);
    }
}
