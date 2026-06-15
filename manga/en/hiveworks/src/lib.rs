use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: Hiveworks = Hiveworks;
const BASE_URL: &str = "https://hiveworkscomics.com";

struct Hiveworks;

impl MangaSource for Hiveworks {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false, ""));
        }
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/home/update-day/{}", current_weekday())
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            false,
            "",
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://") || query.starts_with("https://") {
            return Ok(Paged {
                entries: vec![details_for_url(query)],
                has_next_page: false,
            });
        }
        let target = filtered_url(request.get("filters"));
        let originals = filter_bool(request.get("filters"), "originals");
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, originals, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| SAMPLE_COMIC.to_string());
        Ok(details_for_url(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| SAMPLE_COMIC.to_string());
        let archive = chapter_archive_url(&key);
        let body = fetch_document(&archive, ARCHIVE_FIXTURE);
        let chapters = if archive.contains("witchycomic") {
            parse_witchy_chapters(&body)
        } else if archive.contains("sssscomic") {
            parse_ssss_chapters(&body, &archive)
        } else if archive.contains("awkwardzombie") {
            parse_awkward_zombie_chapters(&body)
        } else {
            parse_standard_chapters(&body, &archive)
        };
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| SAMPLE_CHAPTER.to_string());
        Ok(parse_pages(&fetch_document(&key, PAGES_FIXTURE), &key))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE), false, "");
        let latest = parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/home/update-day/{}", current_weekday()),
                LIST_FIXTURE,
            ),
            false,
            "",
        );
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with("http://") || input.starts_with("https://") {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_url(input)),
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

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer.to_string())
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    let referer = format!("{}/", target_origin(target).unwrap_or(BASE_URL));
    client(&referer)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, originals: bool, query: &str) -> Paged<CatalogItem> {
    let mut entries = if originals {
        class_blocks(body, "originalsblock")
            .into_iter()
            .filter_map(parse_original_block)
            .collect::<Vec<_>>()
    } else {
        class_blocks(body, "comicblock")
            .into_iter()
            .filter_map(parse_comic_block)
            .filter(|item| {
                let item_url = item.url.as_deref().unwrap_or_default();
                !item_url.contains("sparklermonthly.com") && !item_url.contains("explosm.net")
            })
            .collect::<Vec<_>>()
    };
    if !query.is_empty() {
        let needle = query.to_ascii_lowercase();
        entries.retain(|item| item.title.to_ascii_lowercase().contains(&needle));
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_for_url(input: &str) -> CatalogItem {
    let body = fetch_document(BASE_URL, LIST_FIXTURE);
    class_blocks(&body, "comicblock")
        .into_iter()
        .filter_map(parse_comic_block)
        .find(|item| item.url.as_deref() == Some(input))
        .unwrap_or_else(|| CatalogItem {
            key: input.to_string(),
            title: url::slug_from_url(input).unwrap_or_else(|| "Comic".to_string()),
            url: Some(input.to_string()),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_comic_block(block: String) -> Option<CatalogItem> {
    let href = html::attr_after(&block, "comiclink", "href")
        .or_else(|| html::attr_after(&block, "<a", "href"))?;
    let title = html::text_between(&block, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Comic".to_string()));
    let author = html::text_between(&block, "<h2", "</h2>")
        .map(|value| {
            html::strip_tags(&value)
                .trim_start_matches("by")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty());
    let description = html::text_between(&block, "description", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let rating = html::text_between(&block, "comicrating", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let key = url::join_url(BASE_URL, &href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_after(&block, "<img").map(|value| url::join_url(BASE_URL, &value)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        description,
        tags: rating.into_iter().collect(),
        status: ItemStatus::Unknown,
        url: Some(key),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_original_block(block: String) -> Option<CatalogItem> {
    let href = html::attr_after(&block, "<a", "href")?;
    let header = html::text_between(&block, "header", "</div>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    let title = header
        .split_once("by")
        .map(|(title, _)| title.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Comic".to_string()));
    let author = header
        .split_once("by")
        .map(|(_, author)| author.trim().to_string())
        .filter(|value| !value.is_empty());
    let key = url::join_url(BASE_URL, &href);
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_after(&block, "<img").map(|value| url::join_url(BASE_URL, &value)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        description: html::text_between(&block, "description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(key),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn filtered_url(filters: Option<&Value>) -> String {
    for (id, path) in [
        ("originals", "originals"),
        ("kids", "kids"),
        ("completed", "completed"),
        ("hiatus", "hiatus"),
    ] {
        if filter_bool(filters, id) {
            return format!("{BASE_URL}/{path}");
        }
    }
    let mut parts = Vec::new();
    for (id, path) in [
        ("updateDay", "update-day"),
        ("rating", "age"),
        ("genre", "genre"),
        ("title", "alpha"),
        ("sort", "sortby"),
    ] {
        if let Some(value) = filter_str(filters, id)
            .filter(|value| value != "all" && value != "none" && !value.is_empty())
        {
            parts.push(path.to_string());
            parts.push(value);
        }
    }
    if parts.is_empty() {
        BASE_URL.to_string()
    } else {
        format!("{BASE_URL}/home/{}", parts.join("/"))
    }
}

fn chapter_archive_url(input: &str) -> String {
    let trimmed = input.trim_end_matches('/');
    if trimmed.contains("sssscomic") {
        format!("{trimmed}?id=archive")
    } else if trimmed.contains("awkwardzombie") {
        format!("{trimmed}/awkward-zombie/archive")
    } else {
        format!("{trimmed}/comic/archive")
    }
}

fn parse_standard_chapters(body: &str, archive_url: &str) -> Vec<MangaChapter> {
    let base = body
        .split("href='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(ToString::to_string)
        .unwrap_or_else(|| archive_url.trim_end_matches("/comic/archive").to_string());
    let mut chapters = body
        .split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            let text = html::strip_tags(chunk);
            let (date, title) = text
                .split_once('-')
                .map(|(date, title)| (date.trim(), title.trim()))
                .unwrap_or(("", text.trim()));
            let chapter_url = url::join_url(&base, &value);
            Some(MangaChapter {
                key: chapter_url.clone(),
                title: Some(title.to_string()),
                url: Some(chapter_url),
                date_uploaded: parse_named_date(date),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if archive_url.contains("checkpleasecomic") {
        chapters.retain(|chapter| {
            chapter
                .title
                .as_deref()
                .is_some_and(|title| title.ends_with("01") || title.ends_with(" 1"))
        });
    }
    chapters.reverse();
    chapters
}

fn parse_witchy_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("page-"))
        .filter_map(|chunk| html::attr(chunk, "href"))
        .enumerate()
        .map(|(index, href)| {
            let chapter_url = url::join_url(BASE_URL, &href);
            MangaChapter {
                key: chapter_url.clone(),
                title: Some(format!("Page {}", index + 1)),
                url: Some(chapter_url),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_ssss_chapters(body: &str, archive_url: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for adventure in 1..=64 {
        let marker = format!("adv{adventure}Div");
        let Some(block) = body.split(&marker).nth(1) else {
            continue;
        };
        let block = block.split("adv").next().unwrap_or(block);
        for chunk in block.split("<a").skip(1) {
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            if !href.contains("page") {
                continue;
            }
            let page = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| (chapters.len() + 1).to_string());
            let chapter_url = resolve_relative(archive_url, &format!("../../{href}"));
            chapters.push(MangaChapter {
                key: chapter_url.clone(),
                title: Some(format!("Adventure {adventure} - Page {page}")),
                url: Some(chapter_url),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            });
        }
    }
    chapters.reverse();
    chapters
}

fn parse_awkward_zombie_chapters(body: &str) -> Vec<MangaChapter> {
    class_blocks(body, "archive-line")
        .into_iter()
        .filter_map(|block| {
            let href = html::attr_after(&block, "<a", "href")?;
            let date_text = html::text_between(&block, "archive-date", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let chapter_number = date_text
                .split('#')
                .nth(1)
                .and_then(|value| value.split(',').next())
                .and_then(|value| value.parse::<f32>().ok());
            let title = html::text_between(&block, "archive-title", "</div>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Page".to_string());
            let game = html::text_between(&block, "archive-game", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let chapter_url = url::join_url(BASE_URL, &href);
            Some(MangaChapter {
                key: chapter_url.clone(),
                title: Some(format!(
                    "#{} {}{}",
                    chapter_number.unwrap_or(0.0),
                    title,
                    game.map(|value| format!(" ({value})")).unwrap_or_default()
                )),
                url: Some(chapter_url),
                chapter_number,
                date_uploaded: parse_short_date(date_text.split(',').nth(1).unwrap_or_default()),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            if chunk.contains("cc-comicbody")
                || chunk.contains("comicnormal")
                || body.contains("cc-comicbody")
            {
                image_from_chunk(chunk)
            } else {
                None
            }
        })
        .enumerate()
        .map(|(index, image)| page(index, &resolve_image_url(page_url, &image), page_url))
        .collect::<Vec<_>>();
    if pages.is_empty() && page_url.contains("sssscomic") {
        if let Some(image) = html::attr_after(body, "comicnormal", "src") {
            pages.push(page(
                0,
                &resolve_relative(page_url, &format!("../../{image}")),
                page_url,
            ));
        }
    }
    pages
}

fn page(index: usize, image: &str, referer: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn class_blocks(input: &str, class_name: &str) -> Vec<String> {
    let marker = format!("class=\"{class_name}");
    input
        .split(&marker)
        .skip(1)
        .map(|chunk| format!("{marker}{chunk}"))
        .collect()
}

fn image_after(input: &str, marker: &str) -> Option<String> {
    input
        .find(marker)
        .and_then(|index| image_from_chunk(&input[index..]))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn filter_str(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_bool(filters: Option<&Value>, key: &str) -> bool {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn current_weekday() -> &'static str {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0);
    match (days + 4) % 7 {
        0 => "sunday",
        1 => "monday",
        2 => "tuesday",
        3 => "wednesday",
        4 => "thursday",
        5 => "friday",
        _ => "saturday",
    }
}

fn parse_named_date(value: &str) -> Option<i64> {
    let parts = value
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = month_number(&parts[0])?;
    let day = parts[1].parse::<i32>().ok()?;
    let year = parts[2].parse::<i32>().ok()?;
    Some(unix_date(year, month, day))
}

fn parse_short_date(value: &str) -> Option<i64> {
    let parts = value.trim().split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = parts[0].parse::<i32>().ok()?;
    let day = parts[1].parse::<i32>().ok()?;
    let mut year = parts[2].parse::<i32>().ok()?;
    if year < 100 {
        year += 2000;
    }
    Some(unix_date(year, month, day))
}

fn month_number(value: &str) -> Option<i32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

fn unix_date(year: i32, month: i32, day: i32) -> i64 {
    (days_from_civil(year, month, day) as i64) * 86_400
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i32 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn target_origin(target: &str) -> Option<&str> {
    let scheme_end = target.find("://")? + 3;
    let rest = &target[scheme_end..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    Some(&target[..scheme_end + host_end])
}

fn resolve_relative(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    let mut parts = base.split('/').collect::<Vec<_>>();
    parts.pop();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.len() > 3 {
                    parts.pop();
                }
            }
            value => parts.push(value),
        }
    }
    parts.join("/")
}

fn resolve_image_url(page_url: &str, image: &str) -> String {
    if image.starts_with("http://") || image.starts_with("https://") {
        image.to_string()
    } else if image.starts_with('/') {
        target_origin(page_url)
            .map(|origin| url::join_url(origin, image))
            .unwrap_or_else(|| url::join_url(page_url, image))
    } else {
        resolve_relative(page_url, image)
    }
}

export_manga_source!(SOURCE);

const SAMPLE_COMIC: &str = "https://sample.hiveworkscomics.com";
const SAMPLE_CHAPTER: &str = "https://sample.hiveworkscomics.com/comic/page-1";

const LIST_FIXTURE: &str = r#"
<div class="comicblock"><a class="comiclink" href="https://sample.hiveworkscomics.com"><img src="/cover.jpg"></a><h1>Sample Comic</h1><h2>by Sample Author</h2><div class="description">A comic.</div><div class="comicrating">Everyone</div></div>
<div class="originalsblock"><a href="https://original.hiveworkscomics.com"><img src="/original.jpg"></a><div class="header">Original Comic by Original Author</div><div class="description">Original description.</div></div>
"#;
const ARCHIVE_FIXTURE: &str = r#"
<script>href='https://sample.hiveworkscomics.com'</script>
<select name="comic"><option value="/comic/page-0">Jan 01, 2024 - Cover</option><option value="/comic/page-1">Feb 01, 2024 - Page 1</option></select>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="cc-comicbody"><img src="https://sample.hiveworkscomics.com/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_and_chapters() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Comic"
        );
        assert_eq!(
            SOURCE
                .chapters(json!({"manga": SAMPLE_COMIC}))
                .unwrap()
                .len(),
            2
        );
    }
}
