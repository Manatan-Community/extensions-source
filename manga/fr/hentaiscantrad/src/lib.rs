use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HentaiScantrad = HentaiScantrad;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://hentai.scantrad-vf.cc",
    lang: "fr",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct HentaiScantrad;

impl MangaSource for HentaiScantrad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.list_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![manga::Madara::parse_details(&body, Some(key), &CONFIG)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(with_chapter_dates(
            manga::Madara::parse_chapters(&body, &key, &CONFIG),
            &body,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body =
            manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: manga::Madara::parse_listing(body, &CONFIG),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let mut item = manga::Madara::parse_details(body, key, &CONFIG);
    item.status = french_status(body).unwrap_or(item.status);
    item
}

fn with_chapter_dates(mut chapters: Vec<MangaChapter>, body: &str) -> Vec<MangaChapter> {
    let dates = chapter_date_texts(body);
    for (chapter, date) in chapters.iter_mut().zip(dates) {
        chapter.date_uploaded = parse_french_date(&date).or(chapter.date_uploaded);
    }
    chapters
}

fn chapter_date_texts(body: &str) -> Vec<String> {
    body.split("chapter-release-date")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let mut params = vec![
        ("s", query.trim().to_string()),
        ("post_type", "wp-manga".to_string()),
    ];
    for (id, param) in [
        ("author", "author"),
        ("artist", "artist"),
        ("year", "release"),
    ] {
        if let Some(value) = filter_str(filters, id).filter(|value| !value.is_empty()) {
            params.push((param, value.to_string()));
        }
    }
    if let Some(order) = filter_str(filters, "order").filter(|value| !value.is_empty()) {
        params.push(("m_orderby", order.to_string()));
    }
    if let Some(genres) = filter_str(filters, "genres").filter(|value| !value.is_empty()) {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(("genre[]", genre.to_string()));
        }
    }
    format!(
        "{}/{}?{}",
        CONFIG.base_url.trim_end_matches('/'),
        page_path,
        params
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn french_status(body: &str) -> Option<ItemStatus> {
    let lower = body.to_ascii_lowercase();
    if lower.contains("terminé") || lower.contains("termine") {
        Some(ItemStatus::Completed)
    } else if lower.contains("hiatus") || lower.contains("pause") {
        Some(ItemStatus::Hiatus)
    } else if lower.contains("en cours") || lower.contains("ongoing") {
        Some(ItemStatus::Ongoing)
    } else {
        None
    }
}

fn parse_french_date(value: &str) -> Option<i64> {
    let clean = value
        .trim()
        .trim_start_matches("le ")
        .replace(',', " ")
        .to_ascii_lowercase();
    dates::parse_ymd(&clean).or_else(|| parse_day_month_year(&clean))
}

fn parse_day_month_year(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let (day, month, year) = if parts[0].chars().all(|ch| ch.is_ascii_digit()) {
        (parts[0], parts[1], parts[2])
    } else {
        (parts[1], parts[0], parts[2])
    };
    let day = day.parse::<u32>().ok()?;
    let month = french_month(month)?;
    let year = year.parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn french_month(value: &str) -> Option<u32> {
    Some(match value {
        "janvier" | "janv" => 1,
        "février" | "fevrier" | "févr" | "fevr" => 2,
        "mars" => 3,
        "avril" | "avr" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" | "juil" => 7,
        "août" | "aout" => 8,
        "septembre" | "sept" => 9,
        "octobre" | "oct" => 10,
        "novembre" | "nov" => 11,
        "décembre" | "decembre" | "déc" | "dec" => 12,
        _ => return None,
    })
}

fn filter_str<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn deeplink_key(input: &str) -> Option<String> {
    input
        .starts_with(CONFIG.base_url)
        .then(|| CONFIG.normalize_manga_key(input))
}

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail manga"><div class="post-title"><h3><a href="https://hentai.scantrad-vf.cc/manga/sample/">Sample</a></h3></div><img src="/cover.jpg"></div>
<div class="nav-previous"><a href="/manga/page/2/">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary"><p>Resume</p></div>
<ul><li class="wp-manga-chapter"><a href="https://hentai.scantrad-vf.cc/manga/sample/chapter-1/">Chapitre 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" data-src="/page1.jpg"></div>"#;
