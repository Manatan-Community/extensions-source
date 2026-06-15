use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: YaoiScan = YaoiScan;
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: "https://yaoiscan.com",
    lang: "en",
    content_rating: "adult",
    manga_path: "manga",
    popular_url_marker: "post-title",
    use_load_more: false,
    latest_enabled: true,
};

struct YaoiScan;

impl MangaSource for YaoiScan {
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        Ok(list_from_body(&manga::Madara::fetch_document_or_fixture(
            &CONFIG,
            &CONFIG.search_url(page, query),
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
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn fetch_madara_chapters(key: &str) -> Vec<MangaChapter> {
    let manga_url = CONFIG.absolute_url(key);
    let body = manga::Madara::fetch_document_or_fixture(&CONFIG, &manga_url, DETAILS_FIXTURE);
    let chapters = parse_chapter_blocks(&body);
    if !chapters.is_empty() {
        return chapters;
    }
    let Some(manga_id) = html::attr_after(&body, "manga-chapters-holder", "data-id") else {
        return manga::Madara::parse_chapters(&body, key, &CONFIG);
    };
    let ajax = manga::Madara::browser_client(&CONFIG)
        .post(format!("{}/wp-admin/admin-ajax.php", CONFIG.base_url))
        .form(&[("action", "manga_get_chapters"), ("manga", &manga_id)])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    let parsed = parse_chapter_blocks(&ajax);
    if parsed.is_empty() {
        manga::Madara::parse_chapters(&body, key, &CONFIG)
    } else {
        parsed
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
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><h3 class="post-title"><a href="/manga/sample/">Sample Manga</a></h3><img src="/cover.jpg"></div>
<div class="nav-previous"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="post-title">Sample Manga</h1><div id="manga-chapters-holder" data-id="1"></div>
<ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>
"#;
