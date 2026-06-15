use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ResetScans = ResetScans;

struct ResetScans;

impl MangaSource for ResetScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: manga::Madara::parse_listing(LIST_FIXTURE, &config),
                has_next_page: manga::Madara::has_next_page(LIST_FIXTURE, &config),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.list_url(page, order),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(config.base_url) {
            let key = config.normalize_manga_key(query);
            let body = manga::Madara::fetch_document_or_fixture(&config, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &config)],
                has_next_page: false,
            });
        }
        let order = filter(request.get("filters"), "order", "");
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &series_url(&config, page, query, order, request.get("filters")),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: manga::Madara::parse_listing(&body, &config),
            has_next_page: manga::Madara::has_next_page(&body, &config),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        Ok(manga::Madara::parse_details(&body, Some(key), &config))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config();
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            DETAILS_FIXTURE,
        );
        let mut chapters = manga::Madara::parse_chapters(&body, &key, &config);
        apply_yearless_dates(&mut chapters, &body);
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = manga::Madara::fetch_document_or_fixture(
            &config,
            &config.absolute_url(&key),
            PAGES_FIXTURE,
        );
        Ok(manga::Madara::parse_pages(&body, &config))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config();
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(config.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(manga::Madara::parse_details(
                    &manga::Madara::fetch_document_or_fixture(&config, input, DETAILS_FIXTURE),
                    Some(config.normalize_manga_key(input)),
                    &config,
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: "https://reset-scans.org",
        lang: "en",
        content_rating: "safe",
        manga_path: "manga",
        popular_url_marker: "post-title",
        use_load_more: true,
        latest_enabled: true,
    }
}

fn series_url(
    config: &MadaraConfig,
    page: u64,
    query: &str,
    order: &str,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for key in ["author", "yearx", "status", "type"] {
        let value = filter(filters, key, "");
        if !value.is_empty() {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    if !order.is_empty() {
        params.push(format!("order={}", url::query_escape(order)));
    }
    format!(
        "{}/{}/?{}",
        config.base_url,
        config.manga_path,
        params.join("&")
    )
}

fn apply_yearless_dates(chapters: &mut [MangaChapter], body: &str) {
    let dates = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter") && !chunk.contains("#"))
        .filter_map(|chunk| {
            html::text_between(chunk, "chapter-release-date", "</")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| parse_dd_mmm(&value))
        })
        .collect::<Vec<_>>();

    let mut current_year = 2026_i64;
    let mut previous_month: Option<i64> = None;
    for (chapter, (day, month)) in chapters.iter_mut().zip(dates) {
        if let Some(previous) = previous_month {
            if month - previous >= 6 {
                current_year -= 1;
            }
        }
        chapter.date_uploaded = Some(approximate_unix_date(current_year, month, day));
        previous_month = Some(month);
    }
}

fn parse_dd_mmm(value: &str) -> Option<(i64, i64)> {
    let (day, month) = value.trim().split_once('-')?;
    let day = day.parse().ok()?;
    let month = match month.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    Some((day, month))
}

fn approximate_unix_date(year: i64, month: i64, day: i64) -> i64 {
    let years = year - 1970;
    let leap_days = ((year - 1) / 4 - 1969 / 4) - ((year - 1) / 100 - 1969 / 100)
        + ((year - 1) / 400 - 1969 / 400);
    let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let leap_adjust = if month > 2 && is_leap_year(year) {
        1
    } else {
        0
    };
    (years * 365 + leap_days + month_days[(month - 1) as usize] + leap_adjust + day - 1) * 86_400
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample Manga</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<ul class="main version-chap"><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01-Jan</span></li></ul>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
