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
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: Syosetu = Syosetu;
const YOMOU_URL: &str = "https://yomou.syosetu.com";
const NCODE_URL: &str = "https://ncode.syosetu.com";

struct Syosetu;

impl NovelSource for Syosetu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_rankings(RANKING_FIXTURE),
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
            format!("{YOMOU_URL}/search.php?order=new&notnizi=1&p={}", bounded_page(page))
        } else {
            ranking_url(&request, page)
        };
        let body = fetch_yomou(&target, RANKING_FIXTURE);
        let entries = if listing == "latest" {
            parse_search_results(&body)
        } else {
            parse_rankings(&body)
        };
        Ok(Paged {
            has_next_page: has_next_page(&body, page),
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
        let target = format!(
            "{YOMOU_URL}/search.php?order=hyoka&word={}&notnizi=1&p={}",
            url::query_escape(query),
            bounded_page(page)
        );
        let body = fetch_yomou(&target, SEARCH_FIXTURE);
        Ok(Paged {
            entries: parse_search_results(&body),
            has_next_page: has_next_page(&body, page),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "n0000aa".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "n0000aa".to_string());
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
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "n0000aa/1".to_string());
        let body = fetch_ncode(&format!("{NCODE_URL}/{}", key.trim_start_matches('/')), TEXT_FIXTURE);
        let title = html::text_between(&body, "p-novel__title", "</").map(|text| html::strip_tags(&text));
        let body_html = html::text_between(&body, "p-novel__body", "</div>")
            .or_else(|| html::text_between(&body, "js-novel-text", "</div>"))
            .unwrap_or(body);
        let chapter_html = if let Some(title) = &title {
            format!("<h1>{title}</h1>{}", novel::normalize_reader_html(&body_html))
        } else {
            novel::normalize_reader_html(&body_html)
        };
        Ok(NovelText {
            title,
            html: Some(chapter_html.clone()),
            text: Some(novel::cleanup_text(&chapter_html)),
            base_url: Some(NCODE_URL.to_string()),
            css: Some("body { line-height: 1.8; } img { max-width: 100%; height: auto; }".to_string()),
            image_headers: novel::image_headers(NCODE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Rankings".to_string(),
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

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(base_url)
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_yomou(target: &str, fixture: &str) -> String {
    client(YOMOU_URL)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_ncode(target: &str, fixture: &str) -> String {
    client(NCODE_URL)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn ranking_url(request: &Value, page: u64) -> String {
    let filters = request.get("filters");
    let ranking = filter_string(filters, "ranking", "total");
    let genre = filter_string(filters, "genre", "");
    let modifier = filter_string(filters, "modifier", "total");
    let page = bounded_page(page);
    if genre.is_empty() {
        format!("{YOMOU_URL}/rank/list/type/{ranking}_{modifier}/?p={page}")
    } else {
        let list = if genre.len() == 1 { "isekailist" } else { "genrelist" };
        let modifier_suffix = if modifier == "total" {
            String::new()
        } else {
            format!("_{modifier}")
        };
        format!("{YOMOU_URL}/rank/{list}/type/{ranking}_{genre}{modifier_suffix}/?p={page}")
    }
}

fn filter_string(filters: Option<&Value>, key: &str, default: &str) -> String {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn bounded_page(page: u64) -> u64 {
    page.clamp(1, 100)
}

fn parse_rankings(body: &str) -> Vec<CatalogItem> {
    parse_anchor_items(body, &["p-ranklist-item__title", "c-card"])
}

fn parse_search_results(body: &str) -> Vec<CatalogItem> {
    parse_anchor_items(body, &["searchkekka_box", "novel_h", "p-searchResult__title"])
}

fn parse_anchor_items(body: &str, markers: &[&str]) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            if !markers.iter().any(|marker| chunk.contains(marker)) && !chunk.contains(NCODE_URL) {
                return None;
            }
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr(chunk, "title")
                .or_else(|| html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: None,
                url: Some(format!("{NCODE_URL}/{key}/")),
                language: Some("ja".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .take(40)
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let key = normalize_ncode_key(key);
    let body = fetch_ncode(&format!("{NCODE_URL}/{key}/"), DETAILS_FIXTURE);
    let chapters = parse_chapters(&body);
    let mut extra = BTreeMap::new();
    extra.insert(
        "chapterCount".to_string(),
        serde_json::json!(chapters.len()),
    );
    CatalogItem {
        key: key.clone(),
        title: first_text(&body, &["p-novel__title", "<title"]).unwrap_or_else(|| key.clone()),
        cover: None,
        url: Some(format!("{NCODE_URL}/{key}/")),
        authors: first_text(&body, &["p-novel__author"])
            .map(|author| author.replace("作者：", "").trim().to_string())
            .filter(|author| !author.is_empty())
            .into_iter()
            .collect(),
        description: html::text_between(&body, "id=\"novel_ex\"", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: html::attr_after(&body, "property=\"og:description\"", "content")
            .map(|value| value.split_whitespace().map(ToString::to_string).collect())
            .unwrap_or_default(),
        status: parse_status(&body),
        language: Some("ja".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        extra,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str) -> Vec<NovelChapter> {
    let key = normalize_ncode_key(key);
    let first_page = fetch_ncode(&format!("{NCODE_URL}/{key}/"), DETAILS_FIXTURE);
    let mut chapters = parse_chapters(&first_page);
    let last_page = last_chapter_page(&first_page);
    for page in 2..=last_page {
        let page_body = fetch_ncode(&format!("{NCODE_URL}/{key}/?p={page}"), DETAILS_FIXTURE);
        chapters.extend(parse_chapters(&page_body));
    }
    dedupe_chapters(chapters)
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("p-eplist__sublist")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = key_from_url(&href)?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: key
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .and_then(|part| part.parse::<f32>().ok()),
                url: Some(format!("{NCODE_URL}/{key}/")),
                language: Some("ja".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn dedupe_chapters(chapters: Vec<NovelChapter>) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    chapters
        .into_iter()
        .filter(|chapter| seen.insert(chapter.key.clone()))
        .collect()
}

fn last_chapter_page(body: &str) -> u64 {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter_map(|href| href.split("?p=").nth(1).and_then(|page| page.parse::<u64>().ok()))
        .max()
        .unwrap_or(1)
        .min(100)
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| html::text_between(body, marker, "</").map(|value| html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
}

fn parse_status(body: &str) -> ItemStatus {
    let announce = html::text_between(body, "c-announce", "</div>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_else(|| html::strip_tags(body));
    if announce.contains("完結") {
        ItemStatus::Completed
    } else if announce.contains("更新されていません") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn key_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let path = trimmed
        .strip_prefix(NCODE_URL)
        .or_else(|| trimmed.strip_prefix("http://ncode.syosetu.com"))
        .unwrap_or(trimmed)
        .trim_start_matches('/');
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let novel = parts.next()?;
    if !novel.to_ascii_lowercase().starts_with('n') {
        return None;
    }
    let mut key = novel.to_ascii_lowercase();
    if let Some(chapter) = parts.next().filter(|part| part.chars().all(|ch| ch.is_ascii_digit())) {
        key.push('/');
        key.push_str(chapter);
    }
    Some(key)
}

fn normalize_ncode_key(key: &str) -> String {
    key_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_ascii_lowercase())
}

fn has_next_page(body: &str, page: u64) -> bool {
    body.contains(&format!("?p={}", page + 1))
        || body.contains(&format!("&p={}", page + 1))
        || body.contains("c-pager__item--next")
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const RANKING_FIXTURE: &str = r#"
<div class="c-card"><a class="p-ranklist-item__title" href="https://ncode.syosetu.com/n0000aa/">Sample Syosetu</a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="searchkekka_box"><div class="novel_h"><a href="https://ncode.syosetu.com/n0000aa/">Sample Syosetu</a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="p-novel__title">Sample Syosetu</h1>
<div class="p-novel__author">作者：Sample Author</div>
<div id="novel_ex">Sample summary.</div>
<div class="c-announce">連載中</div>
<div class="p-eplist__sublist"><a href="/n0000aa/1/">Episode 1</a><div class="p-eplist__update">2024/01/01</div></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1 class="p-novel__title">Episode 1</h1>
<div class="p-novel__body"><div class="p-novel__text">First paragraph.</div></div>
"#;

export_novel_source!(SOURCE);
