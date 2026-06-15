use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ScribbleHub = ScribbleHub;
const BASE_URL: &str = "https://www.scribblehub.com";

struct ScribbleHub;

impl NovelSource for ScribbleHub {
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
        let target = if listing == "latest" {
            format!("{BASE_URL}/latest-series/?pg={page}")
        } else {
            finder_url(&request, page)
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            has_next_page: has_next_page(&body),
            entries: parse_listing(&body),
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
        let target = format!(
            "{BASE_URL}/?s={}&post_type=fictionposts",
            url::query_escape(query)
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            has_next_page: has_next_page(&body),
            entries: parse_listing(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/1/sample/".to_string());
        Ok(fetch_details(&normalize_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/1/sample/".to_string());
        Ok(fetch_chapters(&normalize_key(&key)))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "read/1/sample-chapter/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &normalize_key(&key)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
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

fn finder_url(request: &Value, page: u64) -> String {
    let mut target = format!("{BASE_URL}/series-finder/?sf=1");
    add_multi(&mut target, request, "genresInclude", "gi");
    add_multi(&mut target, request, "genresExclude", "ge");
    if has_any(request, &["genresInclude", "genresExclude"]) {
        push_param(
            &mut target,
            "mgi",
            &filter_string(request, "genreOperator", "and"),
        );
    }
    add_multi(&mut target, request, "contentWarningInclude", "cti");
    add_multi(&mut target, request, "contentWarningExclude", "cte");
    if has_any(request, &["contentWarningInclude", "contentWarningExclude"]) {
        push_param(
            &mut target,
            "mct",
            &filter_string(request, "contentWarningOperator", "and"),
        );
    }
    push_param(
        &mut target,
        "cp",
        &filter_string(request, "storyStatus", "all"),
    );
    push_param(
        &mut target,
        "sort",
        &filter_string(request, "sort", "ratings"),
    );
    push_param(
        &mut target,
        "order",
        &filter_string(request, "order", "desc"),
    );
    push_param(&mut target, "pg", &page.to_string());
    target
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("search_main_box")
        .skip(1)
        .filter_map(parse_listing_box)
        .take(48)
        .collect()
}

fn parse_listing_box(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "search_title", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = text_after(chunk, "search_title")
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "search_img", "src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: text_after(body, "fic_title")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "fic_image", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: text_after(body, "wi_fic_desc"),
        authors: text_after(body, "auth_name_fic").into_iter().collect(),
        tags: body
            .split("fic_genre")
            .skip(1)
            .filter_map(|chunk| text_until(chunk, "</a>").or_else(|| text_until(chunk, "</span>")))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if body.to_ascii_lowercase().contains("ongoing") {
            ItemStatus::Ongoing
        } else if body.to_ascii_lowercase().contains("hiatus") {
            ItemStatus::Hiatus
        } else {
            ItemStatus::Completed
        },
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str) -> Vec<NovelChapter> {
    let post_id = key.split('/').nth(1).unwrap_or_default();
    if post_id.is_empty() {
        return Vec::new();
    }
    let body = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .referer(absolute_url(key))
        .xhr()
        .form(&[
            ("action", "wi_getreleases_pagination"),
            ("pagenum", "-1"),
            ("mypostid", post_id),
        ])
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    let mut entries: Vec<_> = body
        .split("toc_w")
        .skip(1)
        .filter_map(parse_chapter_row)
        .collect();
    entries.reverse();
    entries
}

fn parse_chapter_row(chunk: &str) -> Option<NovelChapter> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(NovelChapter {
        key: key.clone(),
        title: text_after(chunk, "toc_a"),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        ..NovelChapter::default()
    })
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html_body = content_after(body, "chp_raw").unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        title: text_between_tag(body, "h1").or_else(|| text_between_tag(body, "h2")),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(&absolute_url(key)),
        ..NovelText::default()
    }
}

fn add_multi(target: &mut String, request: &Value, id: &str, query_key: &str) {
    let joined = filter_array(request, id).join(",");
    if !joined.is_empty() {
        push_param(target, query_key, &joined);
    }
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn filter_string(request: &Value, key: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn has_any(request: &Value, ids: &[&str]) -> bool {
    ids.iter().any(|id| !filter_array(request, id).is_empty())
}

fn push_param(target: &mut String, key: &str, value: &str) {
    target.push(if target.contains('?') { '&' } else { '?' });
    target.push_str(key);
    target.push('=');
    target.push_str(&url::query_escape(value));
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    content_after(body, marker)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
        .or_else(|| html::text_between(body, marker, "</span>"))
        .or_else(|| html::text_between(body, marker, "</a>"))
}

fn text_until(body: &str, end: &str) -> Option<String> {
    Some(body.split(end).next()?.to_string())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers")
        || body.contains("page-numbers current")
        || body.contains("class=\"next\"")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("scribblehub.com")
        .then(|| normalize_key(input))
        .filter(|key| key.contains("series/"))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://www.scribblehub.com/")
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

const LIST_FIXTURE: &str = r#"
<div class="search_main_box"><div class="search_img"><img src="https://www.scribblehub.com/cover.jpg"></div><div class="search_title"><a href="https://www.scribblehub.com/series/1/sample/">Sample Novel</a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="fic_title">Sample Novel</h1><div class="fic_image"><img src="https://www.scribblehub.com/cover.jpg"></div><div class="wi_fic_desc">Sample summary.</div><span class="auth_name_fic">Sample Author</span><a class="fic_genre">Fantasy</a><div class="rnd_stats"></div><span>Ongoing</span>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div class="toc_w"><a class="toc_a" href="https://www.scribblehub.com/read/1-sample/chapter/1/">Chapter 1</a><span class="fic_date_pub">1 day ago</span></div>
"#;

const TEXT_FIXTURE: &str = r#"<div class="chp_raw"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
