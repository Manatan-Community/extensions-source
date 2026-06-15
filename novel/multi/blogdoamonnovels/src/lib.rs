use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SOURCE: BlogDoAmonNovels = BlogDoAmonNovels;
const BASE_URL: &str = "https://www.blogdoamonnovels.com";
const DEFAULT_COVER: &str = "https://www.blogdoamonnovels.com/favicon.ico";

struct BlogDoAmonNovels;

impl NovelSource for BlogDoAmonNovels {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return self.search(with_query(request, ""));
        }
        if page > 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(BASE_URL, HOME_FIXTURE);
        Ok(Paged {
            entries: parse_popular_posts(&body),
            has_next_page: false,
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
        let page = page(&request);
        let json_url = blogger_summary_url(query, page);
        let body = fetch_document_or_fixture(&json_url, SEARCH_FIXTURE);
        let entries = parse_feed_novels(&body);
        Ok(Paged {
            has_next_page: entries.len() >= 10,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/p/sample.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/p/sample.html".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let details = parse_details(&body, &key);
        Ok(details
            .extra
            .get("chapters")
            .and_then(Value::as_array)
            .map(|chapters| {
                chapters
                    .iter()
                    .filter_map(chapter_from_json)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| parse_chapters_from_details(&body, &details.title)))
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
            .unwrap_or_else(|| "/2024/01/sample-chapter.html".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
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
                title: "Series".to_string(),
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn blogger_summary_url(search_term: &str, page: u64) -> String {
    let max_results = 10;
    let mut params = vec![
        "alt=json".to_string(),
        format!("max-results={max_results}"),
        format!(
            "q={}",
            url::query_escape(&format!("label:Series {search_term}").trim().to_string())
        ),
    ];
    if page > 1 {
        params.push(format!("start-index={}", (page - 1) * max_results + 1));
    }
    format!("{BASE_URL}/feeds/posts/summary?{}", params.join("&"))
}

fn parse_popular_posts(body: &str) -> Vec<CatalogItem> {
    let source = body.split("PopularPosts").nth(1).unwrap_or(body);
    let mut seen = BTreeSet::new();
    source
        .split("<article")
        .skip(1)
        .filter_map(|article| {
            let href = html::attr_after(article, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(article, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(
                key,
                title,
                html::attr_after(article, "<img", "src")
                    .or_else(|| Some(DEFAULT_COVER.to_string())),
                false,
            ))
        })
        .collect()
}

fn parse_feed_novels(json_text: &str) -> Vec<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(json_text) else {
        return Vec::new();
    };
    root.pointer("/feed/entry")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let title = entry.pointer("/title/$t").and_then(Value::as_str)?;
            let href = alternate_href(entry)?;
            let key = normalize_key(&href);
            Some(catalog_item(
                key,
                title.to_string(),
                entry
                    .pointer("/media$thumbnail/url")
                    .and_then(Value::as_str)
                    .map(|cover| cover.replace("/s72-c/", "/w340/"))
                    .or_else(|| Some(DEFAULT_COVER.to_string())),
                false,
            ))
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        text_for_marker(body, "itemprop=\"name\"").unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "itemprop=\"image\"", "src"),
        true,
    );
    item.description = html::text_between(body, "id=\"synopsis\"", "</")
        .map(|value| html::strip_tags(&value.replace("<br>", "\n").replace("<br/>", "\n")))
        .filter(|value| !value.is_empty());
    item.authors = text_after_info_label(body, "Autor").into_iter().collect();
    item.artists = text_after_info_label(body, "Artista").into_iter().collect();
    item.status = parse_status(
        &html::text_between(body, "data-status", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default(),
    );
    item.tags = link_texts_near(body, "Gênero:");

    let chapters = if let Some(category) = category_from_clwd(body) {
        fetch_category_chapters(&category, &item.title)
    } else {
        parse_chapters_from_details(body, &item.title)
    };
    item.extra.insert(
        "chapters".to_string(),
        Value::Array(chapters.iter().map(chapter_to_json).collect()),
    );
    if item.description.is_none() {
        item.description = fallback_summary_from_chapters(body);
    }
    item
}

fn fetch_category_chapters(category: &str, novel_title: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let mut start_index = 1;
    let max_results = 150;
    loop {
        let json_url = format!(
            "{BASE_URL}/feeds/posts/default/-/{}?alt=json&start-index={start_index}&max-results={max_results}",
            url::query_escape(category)
        );
        let body = fetch_document_or_fixture(&json_url, CHAPTERS_FEED_FIXTURE);
        let Ok(root) = serde_json::from_str::<Value>(&body) else {
            break;
        };
        let Some(entries) = root.pointer("/feed/entry").and_then(Value::as_array) else {
            break;
        };
        for entry in entries {
            let mut title = entry
                .pointer("/title/$t")
                .and_then(Value::as_str)
                .unwrap_or("Chapter")
                .to_string();
            if title == novel_title {
                continue;
            }
            if let Some(content) = entry.pointer("/content/$t").and_then(Value::as_str) {
                if let Some(center_title) = html::text_between(content, "<h1", "</h1>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                {
                    title = center_title;
                }
            }
            let Some(href) = alternate_href(entry) else {
                continue;
            };
            chapters.push(NovelChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: entry
                    .pointer("/updated/$t")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                url: Some(href.to_string()),
                language: Some("pt-BR".to_string()),
                ..NovelChapter::default()
            });
        }
        if entries.len() < max_results as usize {
            break;
        }
        start_index += max_results;
    }
    number_reversed_chapters(chapters)
}

fn parse_chapters_from_details(body: &str, novel_title: &str) -> Vec<NovelChapter> {
    let source = body.split("id=\"chapters\"").nth(1).unwrap_or(body);
    let mut chapters: Vec<_> = source
        .split("<chapter")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let title = html::text_between(block, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(href),
                language: Some("pt-BR".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    if chapters.is_empty() {
        chapters = source
            .split("<a")
            .skip(1)
            .filter_map(|block| {
                let href = html::attr(block, "href")?;
                if !href.contains(".html") {
                    return None;
                }
                let title = html::text_between(block, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| novel_title.to_string());
                Some(NovelChapter {
                    key: normalize_key(&href),
                    title: Some(title),
                    url: Some(href),
                    language: Some("pt-BR".to_string()),
                    ..NovelChapter::default()
                })
            })
            .collect();
    }
    number_reversed_chapters(chapters)
}

fn number_reversed_chapters(mut chapters: Vec<NovelChapter>) -> Vec<NovelChapter> {
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let number = index as f32 + 1.0;
        chapter.chapter_number = Some(number);
        let base = chapter
            .title
            .clone()
            .unwrap_or_else(|| "Chapter".to_string());
        chapter.title = Some(format!("{base} - Ch. {}", index + 1));
    }
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = html::text_between(body, "conteudo_teste", "</div>")
        .or_else(|| html::text_between(body, "post-body", "</div>"))
        .unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&remove_empty_paragraphs(&raw));
    NovelText {
        title: text_between_tag(body, "h1"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(BASE_URL.to_string()),
        css: Some("img { max-width: 100%; height: auto; } body { line-height: 1.7; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        next_chapter_key: Some(normalize_key(key)),
        ..NovelText::default()
    }
}

fn remove_empty_paragraphs(input: &str) -> String {
    input
        .split("<p")
        .enumerate()
        .filter_map(|(index, part)| {
            if index == 0 {
                return Some(part.to_string());
            }
            let text = html::strip_tags(part).replace(BASE_URL, "");
            if text.trim().is_empty() && !part.contains("<img") {
                None
            } else {
                Some(format!("<p{part}"))
            }
        })
        .collect()
}

fn alternate_href(entry: &Value) -> Option<&str> {
    entry
        .get("link")?
        .as_array()?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))?
        .get("href")?
        .as_str()
}

fn chapter_to_json(chapter: &NovelChapter) -> Value {
    json!({
        "key": chapter.key,
        "title": chapter.title,
        "chapterNumber": chapter.chapter_number,
        "dateUploaded": chapter.date_uploaded,
        "url": chapter.url,
        "language": chapter.language,
    })
}

fn chapter_from_json(value: &Value) -> Option<NovelChapter> {
    Some(NovelChapter {
        key: value.get("key")?.as_str()?.to_string(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        chapter_number: value
            .get("chapterNumber")
            .and_then(Value::as_f64)
            .map(|number| number as f32),
        date_uploaded: value.get("dateUploaded").and_then(Value::as_i64),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        language: value
            .get("language")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        ..NovelChapter::default()
    })
}

fn category_from_clwd(body: &str) -> Option<String> {
    html::text_between(body, "id=\"clwd\"", "</")
        .and_then(|value| value.split('\'').nth(1).map(ToString::to_string))
        .filter(|value| !value.is_empty())
}

fn fallback_summary_from_chapters(body: &str) -> Option<String> {
    let mut block = html::text_between(body, "id=\"chapters\"", "</div>")?;
    for marker in [
        "<h3",
        "class=\"flex",
        "separator",
        "custom-hero",
        "id=listItem",
    ] {
        if let Some(prefix) = block.split(marker).next() {
            block = prefix.to_string();
        }
    }
    Some(html::strip_tags(&block)).filter(|value| !value.is_empty())
}

fn text_after_info_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|rest| html::text_between(rest, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts_near(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .take(40)
        .collect()
}

fn text_for_marker(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("complet") || lower.contains("finaliz") {
        ItemStatus::Completed
    } else if lower.contains("hiato") || lower.contains("paus") {
        ItemStatus::Hiatus
    } else if lower.contains("cancel") || lower.contains("drop") {
        ItemStatus::Cancelled
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|cover| absolute_url(&cover)),
        url: Some(absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next().unwrap_or(value);
    let mut parts = date.split('-').filter_map(|part| part.parse::<i32>().ok());
    unix_from_ymd(parts.next()?, parts.next()? as u32, parts.next()? as u32)
}

fn unix_from_ymd(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64 * 86_400)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .map(normalize_key)
        .filter(|key| !key.is_empty())
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key)
        .unwrap_or_else(|| "Novel".to_string())
        .replace('-', " ")
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn with_query(mut request: Value, query: &str) -> Value {
    if !request.is_object() {
        request = json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("query".to_string(), Value::String(query.to_string()));
    }
    request
}

const HOME_FIXTURE: &str = r#"
<div class="PopularPosts"><article><h3><a href="https://www.blogdoamonnovels.com/p/sample.html">Sample Novel</a></h3><img src="/cover.jpg"></article></div>
"#;

const SEARCH_FIXTURE: &str = r#"
{"feed":{"entry":[{"title":{"$t":"Sample Novel"},"link":[{"rel":"alternate","href":"https://www.blogdoamonnovels.com/p/sample.html"}],"media$thumbnail":{"url":"https://example.com/s72-c/cover.jpg"}}]}}
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 itemprop="name">Sample Novel</h1>
<img itemprop="image" src="/cover.jpg">
<div id="synopsis">Sample<br>summary.</div>
<div id="extra-info"><dl>Autor<dd>Sample Author</dd></dl><dl>Artista<dd>Sample Artist</dd></dl></div>
<span data-status>Em andamento</span>
<dt>Gênero:</dt><dd><a>Fantasia</a></dd>
<div id="clwd">'SampleCategory'</div>
"#;

const CHAPTERS_FEED_FIXTURE: &str = r#"
{"feed":{"entry":[{"title":{"$t":"Chapter 1"},"link":[{"rel":"alternate","href":"https://www.blogdoamonnovels.com/2024/01/chapter-1.html"}],"updated":{"$t":"2024-01-01T00:00:00-03:00"},"content":{"$t":"<div class=\"conteudo_teste\"><center><h1>Chapter 1</h1></center></div>"}}]}}
"#;

const TEXT_FIXTURE: &str = r#"
<div class="conteudo_teste"><p>Sample chapter text.</p><p>&nbsp;</p></div>
"#;

export_novel_source!(SOURCE);
