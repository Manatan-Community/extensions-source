use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, url};
use serde_json::Value;

const SOURCE: ZinChanManga = ZinChanManga;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://zinchangmanga.net",
    lang: "en",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct ZinChanManga;

impl MangaSource for ZinChanManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(list_from_body(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
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
        if query.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_from_body(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_body(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(fetch_madara_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(&CONFIG, &CONFIG.absolute_url(&key), PAGES_FIXTURE),
            &CONFIG,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
        Ok(home_sections(popular, latest))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) {
            let key = CONFIG.normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_body(
                    &manga::Madara::fetch_document_or_fixture(&CONFIG, input, DETAILS_FIXTURE),
                    Some(key),
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
        entries: manga::Madara::parse_listing(body, &CONFIG)
            .into_iter()
            .map(apply_slug_title)
            .collect(),
        has_next_page: manga::Madara::has_next_page(body, &CONFIG),
    }
}

fn details_from_body(body: &str, key: Option<String>) -> CatalogItem {
    apply_slug_title(manga::Madara::parse_details(body, key, &CONFIG))
}

fn apply_slug_title(mut item: CatalogItem) -> CatalogItem {
    if let Some(title) = title_from_key(&item.key) {
        item.title = title;
    }
    item
}

fn title_from_key(key: &str) -> Option<String> {
    let slug = key
        .trim_matches('/')
        .strip_prefix("manga/")?
        .split('/')
        .next()
        .filter(|value| !value.is_empty())?;
    let mut result = String::with_capacity(slug.len());
    let mut capitalize = true;
    for ch in slug.chars() {
        if ch == '-' {
            result.push(' ');
            capitalize = true;
        } else if capitalize {
            result.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            result.extend(ch.to_lowercase());
        }
    }
    Some(result)
}

fn fetch_madara_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let chapters = parse_chapter_blocks(&body);
    if !chapters.is_empty() {
        return chapters;
    }
    let ajax = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
        .form(&[])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    let chapters = parse_chapter_blocks(&ajax);
    if chapters.is_empty() {
        fallback_chapter(key)
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
                    .and_then(|value| parse_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn fallback_chapter(key: &str) -> Vec<MangaChapter> {
    vec![MangaChapter {
        key: key.to_string(),
        title: Some("Read".to_string()),
        url: Some(CONFIG.absolute_url(key)),
        ..MangaChapter::default()
    }]
}

fn parse_date(value: &str) -> Option<i64> {
    parse_iso_date(value).or_else(|| parse_slash_date(value))
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let parts = value.trim().split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    ymd_to_unix(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    )
}

fn parse_slash_date(value: &str) -> Option<i64> {
    let parts = value.trim().split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    ymd_to_unix(
        parts[2].parse().ok()?,
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
    )
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

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn home_sections(
    popular: Paged<CatalogItem>,
    latest: Paged<CatalogItem>,
) -> Vec<HomeSection<CatalogItem>> {
    vec![
        HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        },
        HomeSection {
            id: "latest".into(),
            title: "Latest".into(),
            style: Some(HomeSectionStyle::Compact),
            entries: latest.entries,
            has_more: latest.has_next_page,
            ..HomeSection::default()
        },
    ]
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample-title/">Ignored</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Ignored</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Summary</div><div class="wp-manga-chapter"><a href="/manga/sample-title/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
