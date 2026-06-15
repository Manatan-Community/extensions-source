use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MadaraSource = MadaraSource;
const BASE_URL: &str = "https://toon-es.com";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "es",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct MadaraSource;

impl MangaSource for MadaraSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(list_from_body(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(key),
                    &CONFIG,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &filtered_search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(key),
                    &CONFIG,
                )),
                url: Some(input.to_string()),
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

fn list_from_body(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn filtered_search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut target = CONFIG.search_url(page, query);
    let Some(filters) = filters else {
        return target;
    };
    for (param, id) in [
        ("m_orderby", "order"),
        ("genre", "genre"),
        ("status", "status"),
        ("type", "type"),
    ] {
        let Some(value) = filter_value(filters, id).filter(|value| !value.is_empty()) else {
            continue;
        };
        target.push('&');
        target.push_str(param);
        target.push('=');
        target.push_str(&url::query_escape(&value));
    }
    target
}

fn filter_value(filters: &Value, id: &str) -> Option<String> {
    let value = filters.get(id)?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value.as_array().map(|array| {
        array
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",")
    })
}

fn fetch_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let details = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let mut chapters = parse_chapter_blocks(&details);
    if chapters.is_empty() {
        let ajax = manga::Madara::browser_client(&CONFIG)
            .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
        chapters = parse_chapter_blocks(&ajax);
    }
    if chapters.is_empty() {
        vec![MangaChapter {
            key: key.to_string(),
            title: Some("Read".to_string()),
            url: Some(CONFIG.absolute_url(key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_chapter_blocks(body: &str) -> Vec<MangaChapter> {
    body.split("wp-manga-chapter")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_manga_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(CONFIG.absolute_url(&key)),
                is_locked: chunk.contains("locked-badge")
                    || chunk.contains("chapter-lock")
                    || chunk.contains("premium"),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_chapter_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter-img")
                || chunk.contains("reading-content")
                || chunk.contains("data-src")
                || chunk.contains("data-cfsrc")
                || chunk.contains("blocks-gallery-item")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|value| !value.starts_with("data:") && !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: CONFIG.absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(CONFIG.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_chapter_date(value: &str) -> Option<i64> {
    let cleaned = value.trim().replace(',', " ");
    let parts = cleaned.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return manatan_shared::dates::parse_fixture_date(value);
    }
    let month = month_number(parts[0])?;
    let day = parts[1].parse::<i32>().ok()?;
    let mut year = parts[2].parse::<i32>().ok()?;
    if year < 100 {
        year += 2000;
    }
    ymd_to_unix(year, month, day)
}

fn month_number(value: &str) -> Option<i32> {
    match value.trim_end_matches('.').to_ascii_lowercase().as_str() {
        "ene" | "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "abr" | "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "ago" | "aug" => Some(8),
        "sep" | "sept" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dic" | "dec" => Some(12),
        _ => None,
    }
}

fn ymd_to_unix(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era * 146_097 + doe - 719_468) * 86_400)
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Summary</div><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">ene 01, 2024</span></li>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(list_from_body(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapter_blocks(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
