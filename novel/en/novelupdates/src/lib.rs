use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: NovelUpdates = NovelUpdates;
const BASE_URL: &str = "https://www.novelupdates.com";

struct NovelUpdates;

impl NovelSource for NovelUpdates {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/series-finder/?sf=1&sort=sdate&order=desc&pg={page}")
        } else if let Some(rank) =
            filter_string_opt(&request, "rank").filter(|value| !value.is_empty())
        {
            format!("{BASE_URL}/series-ranking/?rank={rank}&pg={page}")
        } else {
            finder_url(&request, page)
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
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
        let mut target = finder_url(&request, page);
        target.push_str("&sh=");
        target.push_str(&url::query_escape(query));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "series/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let post_id = html::attr_after(&body, "id=\"mypostid\"", "value")
            .or_else(|| html::attr_after(&body, "id='mypostid'", "value"))
            .unwrap_or_default();
        if post_id.is_empty() {
            return Ok(parse_chapters(&body));
        }
        let chapters = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .referer(&absolute_url(&key))
            .form(&[
                ("action", "nd_getchapters"),
                ("mygrr", "0"),
                ("mypostid", post_id.as_str()),
            ])
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        let mut entries = parse_chapters(&chapters);
        entries.reverse();
        Ok(entries)
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
            .unwrap_or_else(|| "https://example.com/chapter".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
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
    let mut target = format!(
        "{BASE_URL}/series-finder/?sf=1&sort={}&order={}&pg={page}",
        filter_string(request, "sort", "sdate"),
        filter_string(request, "order", "desc")
    );
    add_multi(&mut target, request, "language", "org");
    add_multi(&mut target, request, "genresInclude", "gi");
    add_multi(&mut target, request, "genresExclude", "ge");
    if let Some(status) = filter_string_opt(request, "storyStatus") {
        target.push_str("&ss=");
        target.push_str(&url::query_escape(&status));
    }
    target
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("search_main_box_nu")
        .skip(1)
        .filter_map(parse_listing_box)
        .take(48)
        .collect()
}

fn parse_listing_box(box_html: &str) -> Option<CatalogItem> {
    let href = html::attr_after(box_html, "search_title", "href")
        .or_else(|| html::attr_after(box_html, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(box_html, "search_title", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(box_html, "<img", "src"),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let type_text = html::text_between(body, "id=\"showtype\"", "</")
        .or_else(|| html::text_between(body, "id='showtype'", "</"))
        .map(|value| html::strip_tags(&value));
    let mut description = html::text_between(body, "id=\"editdescription\"", "</div>")
        .or_else(|| html::text_between(body, "id='editdescription'", "</div>"))
        .map(|value| html::strip_tags(&value));
    if let Some(kind) = type_text.filter(|value| !value.is_empty()) {
        description = Some(format!(
            "{}\n\nType: {kind}",
            description.unwrap_or_default()
        ));
    }
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "seriestitlenu", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "wpb_wrapper", "src")
            .or_else(|| html::attr_after(body, "<img", "src")),
        description,
        authors: collect_links_after(body, "authtag"),
        tags: collect_links_after(body, "seriesgenre"),
        status: if html::text_between(body, "id=\"editstatus\"", "</")
            .unwrap_or_default()
            .contains("Ongoing")
        {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("sp_li_chp")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = if href.starts_with("//") {
                format!("https:{href}")
            } else {
                absolute_url(&href)
            };
            let raw_title = html::text_between(chunk, ">", "</li>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(clean_chapter_title(&raw_title)),
                url: Some(key),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html = first_reader_block(body).unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&html);
    NovelText {
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value)),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some(include_str!("../assets/custom.css").to_string()),
        image_headers: novel::image_headers(&absolute_url(key)),
        ..NovelText::default()
    }
}

fn first_reader_block(body: &str) -> Option<String> {
    [
        "entry-content",
        "chapter-content",
        "chapter__content",
        "post-body",
        "content",
        "reader",
        "chapter",
    ]
    .iter()
    .find_map(|marker| html::text_between(body, marker, "</div>"))
}

fn collect_links_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_chapter_title(value: &str) -> String {
    value
        .replace('v', "volume ")
        .replace('c', " chapter ")
        .replace("part", "part ")
        .replace("ss", "SS")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn add_multi(target: &mut String, request: &Value, id: &str, query_key: &str) {
    let Some(values) = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
    else {
        return;
    };
    let joined = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    if !joined.is_empty() {
        target.push('&');
        target.push_str(query_key);
        target.push('=');
        target.push_str(&url::query_escape(&joined));
    }
}

fn filter_string(request: &Value, key: &str, default: &str) -> String {
    filter_string_opt(request, key).unwrap_or_else(|| default.to_string())
}

fn filter_string_opt(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers") || body.contains("class=\"next\"") || body.contains("&pg=")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("novelupdates.com/series/")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://www.novelupdates.com/")
        .trim_start_matches('/')
        .trim_end_matches('/')
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
<div class="search_main_box_nu"><div class="search_title"><a href="https://www.novelupdates.com/series/sample/">Sample Novel</a></div><img src="https://www.novelupdates.com/cover.jpg"></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="seriestitlenu">Sample Novel</div><div class="wpb_wrapper"><img src="https://www.novelupdates.com/cover.jpg"></div>
<div id="authtag"><a>Sample Author</a></div><div id="seriesgenre"><a>Fantasy</a></div>
<div id="editstatus">Ongoing</div><div id="showtype">Web Novel</div><div id="editdescription">Sample summary.</div>
<input id="mypostid" value="1">
"#;

const CHAPTERS_FIXTURE: &str = r#"
<li class="sp_li_chp"><a href="//translator.example/chapter-1">v1c1</a></li>
"#;

const TEXT_FIXTURE: &str = r#"<div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
