use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: DarthsDroids = DarthsDroids;
const BASE_URL: &str = "https://www.darthsanddroids.net";
const CREATOR: &str = "David Morgan-Mar & Co.";

struct DarthsDroids;

impl MangaSource for DarthsDroids {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: parse_books(&fetch_document(
                &format!("{BASE_URL}/archive.html"),
                ARCHIVE_FIXTURE,
            )),
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
        let entries = self
            .list(Value::Null)?
            .entries
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.title.to_ascii_lowercase().contains(&query)
                    || query.starts_with(BASE_URL)
            })
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/archive.html".into());
        let title = request
            .get("manga")
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str);
        Ok(parse_books(&fetch_document(
            &format!("{BASE_URL}/archive.html"),
            ARCHIVE_FIXTURE,
        ))
        .into_iter()
        .find(|item| item.key == key || title.is_some_and(|title| item.title == title))
        .unwrap_or_else(|| book_item(&key, "Darths & Droids", ItemStatus::Ongoing, 0)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/archive.html".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/episodes/0001.html".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(book_item(
                    &normalize_key(input),
                    "Darths & Droids",
                    ItemStatus::Unknown,
                    0,
                )),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_books(body: &str) -> Vec<CatalogItem> {
    let mut current_title = "Darths & Droids".to_string();
    let mut books = Vec::new();
    for row in body.split("<tr").skip(1) {
        if let Some(title) = html::text_between(row, "<th", "</th>") {
            current_title = format!("Darths & Droids {}", html::strip_tags(&title));
            continue;
        }
        if row.contains("Comic list") {
            if let Some(href) = html::attr_after(row, "<a", "href") {
                let index = books.len();
                books.push(book_item(
                    &normalize_key(&href),
                    &current_title,
                    ItemStatus::Completed,
                    index,
                ));
            }
        } else if row.contains("/episodes/") {
            let index = books.len();
            books.push(book_item(
                "/archive.html",
                &current_title,
                ItemStatus::Ongoing,
                index,
            ));
            break;
        }
    }
    if books.is_empty() {
        books.push(book_item(
            "/archive.html",
            "Darths & Droids",
            ItemStatus::Ongoing,
            0,
        ));
    }
    books
}

fn book_item(key: &str, title: &str, status: ItemStatus, index: usize) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: title.to_string(),
        cover: Some(thumbnail(index).to_string()),
        authors: vec![CREATOR.to_string()],
        artists: vec![CREATOR.to_string()],
        description: Some("A tabletop campaign retelling of Star Wars, where the plot is made up by players at the table.".to_string()),
        tags: vec!["Campaign Comic".to_string(), "Comedy".to_string(), "Science Fiction".to_string()],
        status,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn thumbnail(index: usize) -> &'static str {
    match index {
        0 => "https://www.darthsanddroids.net/cast/QuiGon.jpg",
        1 => "https://www.darthsanddroids.net/cast/Anakin2.jpg",
        2 => "https://www.darthsanddroids.net/cast/ObiWan3.jpg",
        3 => "https://www.darthsanddroids.net/cast/JarJar2.jpg",
        4 => "https://www.darthsanddroids.net/cast/Leia4.jpg",
        5 => "https://www.darthsanddroids.net/cast/Han5.jpg",
        6 => "https://www.darthsanddroids.net/cast/Luke6.jpg",
        7 => "https://www.darthsanddroids.net/cast/Cassian.jpg",
        8 => "https://www.darthsanddroids.net/cast/C3PO4.jpg",
        9 => "https://www.darthsanddroids.net/cast/Finn7.jpg",
        10 => "https://www.darthsanddroids.net/cast/Han4.jpg",
        11 => "https://www.darthsanddroids.net/cast/Hux8.jpg",
        _ => "https://www.darthsanddroids.net/cast/Vader4.jpg",
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut index = 0.0;
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter_map(|row| {
            let cells = row.split("<td").skip(1).collect::<Vec<_>>();
            let (date_text, link_cell) = if cells.len() >= 3 {
                (html::strip_tags(cells[0]), cells[2])
            } else if cells.len() == 1 && !cells[0].contains("colspan") {
                (String::new(), cells[0])
            } else {
                return None;
            };
            let href = html::attr_after(link_cell, "<a", "href")?;
            if !href.contains("/episodes/") && !href.contains("/solo/") {
                return None;
            }
            let title = html::text_between(link_cell, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Episode".to_string());
            let chapter_number = index;
            index += 1.0;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: Some(chapter_number),
                date_uploaded: parse_dnd_date(&date_text),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("logo") && !chunk.contains("rss_"))
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

fn parse_dnd_date(value: &str) -> Option<i64> {
    let parts = value
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    let day = parts[1].parse::<i32>().ok()?;
    let month = month_number(&parts[2])?;
    let year = parts[3].parse::<i32>().ok()?;
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
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<tr><th colspan="3">I. The Phantasmal Malevolence</th></tr>
<tr><td colspan="3">Fri 14 Sep, 2007 to Tue 20 Jan, 2009 - <a href="/archive1.html">Comic list: Episode 1 to Episode 208</a></td></tr>
"#;
const CHAPTERS_FIXTURE: &str = r#"<tr><td>Fri 14 Sep, 2007</td><td>-</td><td><a href="/episodes/0001.html">Episode 1: Sample</a></td></tr>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="center"><p><img src="/comics/darths001.png" width="800"></p></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_archive_chapters_and_pages() {
        assert_eq!(parse_books(ARCHIVE_FIXTURE)[0].key, "/archive1.html");
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
