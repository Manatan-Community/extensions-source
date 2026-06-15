use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, Paged, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: TopManhuaFan = TopManhuaFan;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://www.topmanhua.fan",
    lang: "en",
    content_rating: "safe",
    manga_path: "manhua",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct TopManhuaFan;

impl MangaSource for TopManhuaFan {
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
        if query.starts_with(CONFIG.base_url) {
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
            &CONFIG.search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhua/sample".into());
        Ok(manga::Madara::parse_details(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                DETAILS_FIXTURE,
            ),
            Some(key),
            &CONFIG,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manhua/sample".into());
        Ok(fetch_madara_chapters(&key, false))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manhua/sample/chapter-1".into());
        Ok(manga::Madara::parse_pages(
            &manga::Madara::fetch_document_or_fixture(
                &CONFIG,
                &CONFIG.absolute_url(&key),
                PAGES_FIXTURE,
            ),
            &CONFIG,
        ))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(CONFIG.base_url) {
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

fn fetch_madara_chapters(key: &str, use_new_endpoint: bool) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let chapters = parse_chapter_blocks(&body);
    if !chapters.is_empty() {
        return chapters;
    }
    let Some(manga_id) = html::attr_after(&body, "manga-chapters-holder", "data-id") else {
        return chapters;
    };
    let mut ajax = fetch_chapter_ajax(&manga_url, &manga_id, use_new_endpoint);
    let mut parsed = parse_chapter_blocks(&ajax);
    if parsed.is_empty() && !use_new_endpoint {
        ajax = fetch_chapter_ajax(&manga_url, &manga_id, true);
        parsed = parse_chapter_blocks(&ajax);
    }
    parsed
}

fn fetch_chapter_ajax(manga_url: &str, manga_id: &str, use_new_endpoint: bool) -> String {
    if use_new_endpoint {
        manga::Madara::browser_client(&CONFIG)
            .post(format!("{}/ajax/chapters", manga_url.trim_end_matches('/')))
            .form(&[])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string())
    } else {
        manga::Madara::browser_client(&CONFIG)
            .post(format!(
                "{}/wp-admin/admin-ajax.php",
                CONFIG.base_url.trim_end_matches('/')
            ))
            .form(&[("action", "manga_get_chapters"), ("manga", manga_id)])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| DETAILS_FIXTURE.to_string())
    }
}

fn parse_chapter_blocks(body: &str) -> Vec<MangaChapter> {
    body.split("wp-manga-chapter")
        .skip(1)
        .filter_map(|chunk| {
            if chunk.contains("premium-block") {
                return None;
            }
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
                    .and_then(|value| parse_slash_date(&value, false)),
                ..MangaChapter::default()
            })
        })
        .filter(|chapter| !chapter.is_locked)
        .collect()
}

fn parse_slash_date(value: &str, short_year: bool) -> Option<i64> {
    let parts = value.trim().split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = parts[0].parse::<i32>().ok()?;
    let day = parts[1].parse::<i32>().ok()?;
    let mut year = parts[2].parse::<i32>().ok()?;
    if short_year && year < 100 {
        year += 2000;
    }
    ymd_to_unix(year, month, day)
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

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><h3 class="post-title"><a href="/manhua/sample/">Sample</a></h3><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">Sample</h1><div class="summary_image"><img src="/cover.jpg"></div><div class="description-summary">Summary</div><div class="wp-manga-chapter"><a href="/manhua/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
