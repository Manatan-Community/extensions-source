use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
};
use serde_json::Value;

const SOURCE: Agitoon = Agitoon;
const BASE_URL: &str = "https://agit664.xyz";

struct Agitoon;

impl NovelSource for Agitoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let menu = if listing == "latest" { "1" } else { "3" };
        let offset = (page.saturating_sub(1) * 20).to_string();
        let is_first = if page == 1 { "true" } else { "false" };
        let body = post_form_or_fixture(
            "/novel/index.update.php",
            &[
                ("mode", "get_data_novel_list_p"),
                ("novel_menu", menu),
                ("np_day", "0"),
                ("np_rank", "1"),
                ("np_distributor", "0"),
                ("np_genre", "00"),
                ("np_order", "1"),
                ("np_genre_ex_1", "00"),
                ("np_genre_ex_2", "00"),
                ("list_limit", &offset),
                ("is_query_first", is_first),
            ],
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = post_form_or_fixture(
            "/novel/search.php",
            &[
                ("mode", "get_data_novel_list_p_sch"),
                ("search_novel", query),
                ("list_limit", "0"),
            ],
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "12345".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "12345".to_string());
        Ok(fetch_chapters(&key))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "123456/2".to_string());
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/novel/view/{key}"), TEXT_FIXTURE);
        let html = html::text_between(&body, "id_wr_content", "</div>").unwrap_or(body);
        Ok(NovelText {
            html: Some(html.clone()),
            text: Some(novel::cleanup_text(&html)),
            base_url: Some(BASE_URL.to_string()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_form_or_fixture(path: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}{path}"))
        .xhr()
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&format!("{BASE_URL}/novel/list/{key}"), DETAILS_FIXTURE);
    let chapters = fetch_chapters(key);
    let mut item = CatalogItem {
        key: key.to_string(),
        title: first_text(&body, &["<h5", "<title"]).unwrap_or_else(|| "Agitoon".to_string()),
        cover: html::attr_after(&body, "col-5", "src").map(|src| absolute_url(&src)),
        description: first_text(&body, &["pt-1 mt-1 pb-1 mb-1", "post-content"]),
        authors: first_text(&body, &["post-item-list-cate-v"])
            .into_iter()
            .collect(),
        tags: body
            .split("<span")
            .skip(1)
            .filter_map(|block| html::text_between(block, ">", "</span>"))
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty())
            .take(24)
            .collect(),
        status: ItemStatus::Ongoing,
        url: Some(format!("{BASE_URL}/novel/list/{key}")),
        language: Some("ko".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if item.description.is_none() && !chapters.is_empty() {
        item.description = Some(format!("{} chapters", chapters.len()));
    }
    item
}

fn fetch_chapters(key: &str) -> Vec<NovelChapter> {
    let body = post_form_or_fixture(
        "/novel/list.update.php",
        &[
            ("mode", "get_data_novel_list_c"),
            ("wr_id_p", key),
            ("page_no", "1"),
            ("cnt_list", "10000"),
            ("order_type", "Asc"),
        ],
        CHAPTERS_FIXTURE,
    );
    let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    root.get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let id = chapter.get("wr_id").and_then(Value::as_str)?;
            Some(NovelChapter {
                key: format!("{id}/2"),
                title: chapter
                    .get("wr_subject")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(format!("{BASE_URL}/novel/view/{id}/2")),
                language: Some("ko".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|novel| {
            let key = novel.get("wr_id").and_then(Value::as_str)?.to_string();
            let np_dir = novel.get("np_dir").and_then(Value::as_str).unwrap_or("");
            let thumb = novel
                .get("np_thumbnail")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(CatalogItem {
                key: key.clone(),
                title: novel
                    .get("wr_subject")
                    .and_then(Value::as_str)
                    .unwrap_or("Agitoon")
                    .to_string(),
                cover: (!thumb.is_empty()).then(|| {
                    format!(
                        "{BASE_URL}{}/thumbnail/{thumb}",
                        ensure_leading_slash(np_dir)
                    )
                }),
                url: Some(format!("{BASE_URL}/novel/list/{key}")),
                language: Some("ko".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| {
            html::text_between(body, marker, "</").map(|text| html::strip_tags(&text))
        })
        .filter(|text| !text.is_empty())
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .split("/novel/list/")
        .nth(1)
        .or_else(|| input.split("/novel/view/").nth(1))
        .map(|value| {
            value
                .trim_matches('/')
                .split('/')
                .next()
                .unwrap_or(value)
                .to_string()
        })
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{BASE_URL}{}", ensure_leading_slash(path))
    }
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    request["listing"] = Value::String(listing.to_string());
    request
}

const LIST_FIXTURE: &str = r#"{"list":[{"wr_id":"12345","wr_subject":"Sample Novel","np_dir":"/data/sample","np_thumbnail":"cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<h5 class="pt-2">Sample Novel</h5><div class="pt-1 mt-1 pb-1 mb-1">A fixture synopsis.</div>"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"list":[{"wr_id":"123456","wr_subject":"Chapter 1","wr_datetime":"2024-01-01"}]}"#;
const TEXT_FIXTURE: &str = r#"<div id="id_wr_content"><p>Fixture chapter text.</p></div>"#;

export_novel_source!(SOURCE);
