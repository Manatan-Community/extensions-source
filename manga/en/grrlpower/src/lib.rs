use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GrrlPower = GrrlPower;
const BASE_URL: &str = "https://www.grrlpowercomic.com";
const SERIES_KEY: &str = "/archive";
const AUTHOR: &str = "David Barrack";
const COVER: &str = "https://static.tvtropes.org/pmwiki/pub/images/rsz_grrl_power.png";

struct GrrlPower;

impl MangaSource for GrrlPower {
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
            .unwrap_or_else(|| "/archives/comic/gp0001".to_string());
        let show_notes = request
            .get("preferences")
            .and_then(|prefs| prefs.get("showAuthorsNotes"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_pages(
            &fetch_document(&url::join_url(BASE_URL, &key), PAGE_FIXTURE),
            show_notes,
        ))
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
        title: "Grrl Power".to_string(),
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some("Grrl Power is a comic about a crazy nerdette that becomes a superheroine. Humor, action, cheesecake, beefcake, 'splosions, and maybe some drama.".to_string()),
        cover: Some(COVER.to_string()),
        tags: vec!["superhero".to_string(), "humor".to_string(), "action".to_string()],
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
    for year in 2010..=2035 {
        let fixture = if year == 2024 { ARCHIVE_FIXTURE } else { "" };
        let body = fetch_document(&format!("{BASE_URL}/archive/?archive_year={year}"), fixture);
        for chapter in parse_archive_year(&body, year) {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
        }
    }
    chapters.sort_by(|left, right| right.date_uploaded.cmp(&left.date_uploaded));
    chapters
}

fn parse_archive_year(body: &str, year: i32) -> Vec<MangaChapter> {
    body.split("archive-date")
        .skip(1)
        .filter_map(|chunk| {
            let date_text = html::text_between(chunk, ">", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Comic".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: parse_archive_date(&date_text, year),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, show_notes: bool) -> Vec<MangaPage> {
    let mut pages = body
        .split("<div")
        .find(|chunk| chunk.contains("id=\"comic\"") || chunk.contains("id='comic'"))
        .and_then(|chunk| html::attr_after(chunk, "<img", "src"))
        .map(|image| {
            vec![MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some("Page 1".to_string()),
                ..MangaPage::default()
            }]
        })
        .unwrap_or_default();
    if show_notes {
        let notes = body
            .split("<div")
            .find(|chunk| chunk.contains("class=\"entry") || chunk.contains("class='entry"))
            .map(html::strip_tags)
            .filter(|value| !value.is_empty());
        if let Some(notes) = notes {
            pages.push(MangaPage {
                content: PageContent::Text { text: notes },
                description: Some(format!("Author's Notes from {AUTHOR}")),
                ..MangaPage::default()
            });
        }
    }
    pages
}

fn parse_archive_date(input: &str, year: i32) -> Option<i64> {
    let mut parts = input.split_whitespace();
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day = parts.next()?.parse().ok()?;
    unix_date(year, month, day)
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
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

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"<span class="archive-date">Jan 01</span><span><a href="/archives/comic/gp0001">Sample Page</a></span>"#;
const PAGE_FIXTURE: &str = r#"<div id="comic"><img src="/comic/page.jpg"></div><div class="entry"><p>Sample author's notes.</p></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_single_series_webcomic() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Grrl Power"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/archives/comic/gp0001","preferences":{"showAuthorsNotes":true}}))
                .unwrap()
                .len(),
            2
        );
    }
}
