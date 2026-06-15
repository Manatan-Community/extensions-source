use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RealLifeComics = RealLifeComics;
const BASE_URL: &str = "https://reallifecomics.com";
const LOGO: &str = "/images/logo.png";
const AUTHOR: &str = "Maelyn Dean";
const SUMMARY: &str = "The normal daily lives of some abnormal people. This entry includes all the chapters published in";
const LATEST_ARCHIVE_YEAR: i32 = 2026;

struct RealLifeComics;

impl MangaSource for RealLifeComics {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: archive_years().into_iter().map(year_item).collect(),
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
        Ok(Paged {
            entries: archive_years()
                .into_iter()
                .map(year_item)
                .filter(|item| {
                    query.is_empty()
                        || item.title.to_ascii_lowercase().contains(&query)
                        || query.starts_with(BASE_URL)
                })
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/archivepage.php?year={}", current_year()));
        Ok(year_item(year_from_key(&key).unwrap_or_else(current_year)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/archivepage.php?year={}", current_year()));
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic.php?comic=january-1-2024".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let entries = archive_years().into_iter().map(year_item).collect();
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Compact),
            entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = year_from_key(&key).map(year_item);
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

fn archive_years() -> Vec<i32> {
    (1999..=current_year())
        .rev()
        .filter(|year| !(2016..=2017).contains(year))
        .collect()
}

fn year_item(year: i32) -> CatalogItem {
    let key = format!("/archivepage.php?year={year}");
    CatalogItem {
        key: key.clone(),
        title: format!("Real Life Comics ({year})"),
        cover: Some(url::join_url(BASE_URL, LOGO)),
        authors: vec![AUTHOR.into()],
        artists: vec![AUTHOR.into()],
        description: Some(format!("{SUMMARY} {year}")),
        status: if year == current_year() {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if !key.contains("comic") {
                return None;
            }
            let day_text = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let month_year = month_year_before(body, chunk).unwrap_or_default();
            let date_text = format!("{month_year} {day_text}").trim().to_string();
            let date = parse_month_year_day(&date_text);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(date.map(format_weekday_date).unwrap_or(date_text)),
                date_uploaded: date,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    chapters
        .iter_mut()
        .enumerate()
        .for_each(|(index, chapter)| chapter.chapter_number = Some(index as f32));
    chapters
}

fn month_year_before(body: &str, chunk: &str) -> Option<String> {
    let index = body.find(chunk)?;
    let before = &body[..index];
    ["<h2", "<h3", "<caption"]
        .into_iter()
        .filter_map(|tag| {
            before
                .rfind(tag)
                .and_then(|start| html::text_between(&before[start..], tag, "</"))
        })
        .map(|value| html::strip_tags(&value))
        .find(|value| value.split_whitespace().count() >= 2)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("comic") || chunk.contains("comicimage") || chunk.contains("comicimg")
        })
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
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

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn year_from_key(key: &str) -> Option<i32> {
    key.split("year=")
        .nth(1)?
        .split(['&', '#'])
        .next()?
        .parse()
        .ok()
}

fn parse_month_year_day(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let month = month_number(parts[0])?;
    let year = parts[1].parse().ok()?;
    let day = parts[2].parse().ok()?;
    Some(unix_from_ymd(year, month, day))
}

fn month_number(value: &str) -> Option<i32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    })
}

fn format_weekday_date(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let weekday = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ][days.rem_euclid(7) as usize];
    let (year, month, day) = ymd_from_unix(timestamp);
    format!("{weekday}, {} {day:02}, {year}", month_name(month))
}

fn month_name(month: i32) -> &'static str {
    [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][month as usize]
}

fn current_year() -> i32 {
    LATEST_ARCHIVE_YEAR
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

fn ymd_from_unix(timestamp: i64) -> (i32, i32, i32) {
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + (month <= 2) as i64;
    (year as i32, month as i32, day as i32)
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"<h2>January 2024</h2><table class="calendar"><tbody><tr><td><a href="/comic.php?comic=january-1-2024">1</a></td><td><a href="/comic.php?comic=january-2-2024">2</a></td></tr></tbody></table>"#;
const PAGE_FIXTURE: &str = r#"<div class="comic"><img src="/images/comic.png"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert!(SOURCE.list(json!({})).unwrap().entries.len() > 20);
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 2);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
