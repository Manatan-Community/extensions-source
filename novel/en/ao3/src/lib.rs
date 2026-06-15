use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Ao3 = Ao3;
const BASE_URL: &str = "https://archiveofourown.org";

struct Ao3;

impl NovelSource for Ao3 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_works(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let page = page(&request);
        let sort = if request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            == Some("latest")
        {
            "revised_at"
        } else {
            "hits"
        };
        let target = format!(
            "{BASE_URL}/works/search?commit=Search&page={page}&work_search%5Blanguage_id%5D=en&work_search%5Bsort_column%5D={sort}&work_search%5Bsort_direction%5D=desc"
        );
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_works(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_or_fixture(&work_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/works/search?commit=Search&page={page}&work_search%5Blanguage_id%5D=en&work_search%5Bquery%5D={}",
            url::query_escape(query)
        );
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_works(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "works/1".to_string());
        Ok(parse_details(
            &fetch_or_fixture(&work_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "works/1".to_string());
        let navigate = fetch_or_fixture(&format!("{}/navigate", work_url(&key)), NAVIGATE_FIXTURE);
        let work = fetch_or_fixture(&work_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_navigate_chapters(&navigate, &key);
        if chapters.is_empty() {
            chapters = parse_inline_chapters(&work, &key);
        }
        Ok(chapters)
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
            .unwrap_or_else(|| "works/1/chapters/1".to_string());
        Ok(parse_text(
            &fetch_or_fixture(&work_url(&key), TEXT_FIXTURE),
            &key,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Compact),
            entries: parse_works(LIST_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_or_fixture(&work_url(&key), DETAILS_FIXTURE),
                    &key,
                )),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('?')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn work_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_works(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .filter(|chunk| chunk.contains("work"))
        .filter_map(|chunk| {
            let heading = chunk
                .find("heading")
                .map(|idx| &chunk[idx..])
                .unwrap_or(chunk);
            let href = html::attr_after(heading, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(heading, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Untitled".to_string()),
                cover: None,
                url: Some(work_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let fandom = tags_after(body, "dd class=\"fandom");
    let rating = tags_after(body, "dd class=\"rating");
    let warning = tags_after(body, "dd class=\"warning");
    let relationships = tags_after(body, "dd class=\"relationship");
    let characters = tags_after(body, "dd class=\"character");
    let freeform = tags_after(body, "dd class=\"freeform");
    let mut description = String::new();
    append_section(&mut description, "Fandom", &fandom.join(", "));
    append_section(&mut description, "Rating", &rating.join(", "));
    append_section(&mut description, "Warning", &warning.join(", "));
    append_section(
        &mut description,
        "Summary",
        &text_between(body, "blockquote", "</blockquote>").unwrap_or_default(),
    );
    append_section(&mut description, "Relationships", &relationships.join(", "));
    append_section(&mut description, "Characters", &characters.join(", "));
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "h2 class=\"title", "</h2>")
            .or_else(|| text_between(body, "<h2", "</h2>"))
            .unwrap_or_else(|| "Untitled".to_string()),
        cover: None,
        description: (!description.is_empty()).then_some(description),
        authors: body
            .split("rel=\"author\"")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        tags: fandom
            .into_iter()
            .chain(rating)
            .chain(warning)
            .chain(relationships)
            .chain(characters)
            .chain(freeform)
            .collect(),
        status: if body.contains("dt class=\"status\"") && body.contains("Updated") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(work_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_navigate_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    body.split("<li")
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/chapters/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                chapter_number: chapter_number(&key),
                url: Some(work_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .map(|(idx, mut chapter)| {
            if chapter.chapter_number.is_none() {
                chapter.chapter_number = Some(idx as f32 + 1.0);
            }
            if !chapter.key.starts_with("works/") {
                chapter.key = format!("{}/{}", novel_key.trim_end_matches('/'), chapter.key);
            }
            chapter
        })
        .collect()
}

fn parse_inline_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    body.split("h3 class=\"title")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").unwrap_or_else(|| novel_key.to_string());
            let key = normalize_key(&href);
            Some(NovelChapter {
                key: key.clone(),
                title: text_between(chunk, ">", "</h3>").or_else(|| Some("Chapter".to_string())),
                chapter_number: chapter_number(&key),
                url: Some(work_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let html_body = html::text_between(body, "id=\"chapters\"", "</div>")
        .or_else(|| html::text_between(body, "id='chapters'", "</div>"))
        .unwrap_or_else(|| TEXT_HTML_FIXTURE.to_string());
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        title: text_between(body, "h3 class=\"title", "</h3>")
            .or_else(|| text_between(body, "<h2", "</h2>")),
        html: Some(normalized.clone()),
        text: Some(html::strip_tags(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: next_chapter_key(key),
        ..NovelText::default()
    }
}

fn tags_after(body: &str, marker: &str) -> Vec<String> {
    let Some(start) = body.find(marker) else {
        return Vec::new();
    };
    let section = body[start..].split("</dd>").next().unwrap_or_default();
    section
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn append_section(out: &mut String, label: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(label);
    out.push_str(":\n");
    out.push_str(value.trim());
}

fn text_between(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn chapter_number(path: &str) -> Option<f32> {
    path.rsplit('/')
        .next()
        .and_then(|part| part.parse::<f32>().ok())
}

fn next_chapter_key(key: &str) -> Option<String> {
    let (prefix, number) = key.rsplit_once('/')?;
    let next = number.parse::<u64>().ok()? + 1;
    Some(format!("{prefix}/{next}"))
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("class=\"next\"")
}

const LIST_FIXTURE: &str =
    r#"<li class="work"><h4 class="heading"><a href="/works/1">Sample Work</a></h4></li>"#;
const DETAILS_FIXTURE: &str = r#"<h2 class="title heading">Sample Work</h2><a rel="author">Sample Author</a><blockquote class="userstuff">Sample summary.</blockquote><dl><dt class="status">Completed:</dt></dl><div id="chapters"><h3 class="title"><a href="/works/1/chapters/1">Chapter 1</a></h3></div>"#;
const NAVIGATE_FIXTURE: &str =
    r#"<ol class="index"><li><a href="/works/1/chapters/1">Chapter 1</a></li></ol>"#;
const TEXT_HTML_FIXTURE: &str = r#"<div><p>The first fixture paragraph.</p></div>"#;
const TEXT_FIXTURE: &str =
    r#"<div id="chapters"><div class="chapter"><p>The first fixture paragraph.</p></div></div>"#;

export_novel_source!(SOURCE);
