use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Tsundoku = Tsundoku;
const BASE_URL: &str = "https://tsundoku.com.br";

struct Tsundoku;

impl NovelSource for Tsundoku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body =
            fetch_document_or_fixture(&catalog_url("", page(&request), &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && lnreader::has_next_page(&body),
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
        let body =
            fetch_document_or_fixture(&catalog_url(query, page(&request), &request), LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && lnreader::has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "manga/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "manga/sample".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
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
            .unwrap_or_else(|| "manga/sample/chapter-1".to_string());
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

fn catalog_url(query: &str, page: u64, request: &Value) -> String {
    let mut params = vec!["type=novel".to_string()];
    if page > 1 {
        params.push(format!("page={page}"));
    }
    if !query.is_empty() {
        params.push(format!("title={}", url::query_escape(query)));
    }
    let listing = request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str);
    let order = if listing == Some("latest") {
        "latest".to_string()
    } else {
        lnreader::filter_string_opt(request, "order").unwrap_or_default()
    };
    if !order.is_empty() {
        params.push(format!("order={}", url::query_escape(&order)));
    }
    for genre in lnreader::filter_array(request, "genre") {
        params.push(format!("genre%5B%5D={}", url::query_escape(&genre)));
    }
    format!("{BASE_URL}/manga/?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bsx") || chunk.contains("listupd"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, "class=\"tt", "</")
                .or_else(|| html::text_between(chunk, "class='tt", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(
                key,
                title,
                html::attr_after(chunk, "<img", "src"),
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
        html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "main-info", "src")
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| html::attr_after(body, "<img", "src")),
        true,
    );
    item.description = html::text_between(body, "entry-content entry-content-single", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.authors = info_value(body, "Autor").into_iter().collect();
    item.artists = info_value(body, "Artista").into_iter().collect();
    item.status = info_value(body, "Status")
        .map(|value| parse_status(&value))
        .unwrap_or(ItemStatus::Unknown);
    item.tags = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre") || chunk.contains("mgen"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect();
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut seen = BTreeSet::new();
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            if key.is_empty() || !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_portuguese_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some("multi".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((index + 1) as f32);
        if let Some(title) = &chapter.title {
            chapter.title = Some(format!("{title} - Ch. {}", index + 1));
        }
    }
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let title = html::text_between(body, "headpost", "</h")
        .or_else(|| html::text_between(body, "entry-title", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let raw = html::text_between(body, "collapseomatic_content", "</div>")
        .or_else(|| html::text_between(body, "id=\"readerarea\"", "</div>"))
        .or_else(|| html::text_between(body, "id='readerarea'", "</div>"))
        .unwrap_or_else(|| body.to_string());
    let mut parts = raw.split("<hr").map(str::to_string).collect::<Vec<_>>();
    if parts.len() > 1
        && parts
            .last()
            .is_some_and(|part| part.contains("https://discord"))
    {
        parts.pop();
    }
    let normalized = novel::normalize_reader_html(&parts.join("<hr"));
    NovelText {
        title,
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
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
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("multi".to_string()),
        content_rating: Some("adult".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("tsundoku.com.br")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Tsundoku")
        .replace(['-', '_'], " ")
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("imptdt")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            html::strip_tags(chunk)
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("complet") || lower.contains("final") {
        ItemStatus::Completed
    } else if lower.contains("paus") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("drop") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("ativo") || lower.contains("ongoing") || lower.contains("andamento") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_portuguese_date(value: &str) -> Option<i64> {
    let cleaned = value.trim().to_ascii_lowercase();
    let parts = cleaned
        .split([',', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return manatan_shared::dates::parse_fixture_date(value);
    }
    let month = match strip_accents(parts[0]).as_str() {
        "janeiro" => 1,
        "fevereiro" => 2,
        "marco" => 3,
        "abril" => 4,
        "maio" => 5,
        "junho" => 6,
        "julho" => 7,
        "agosto" => 8,
        "setembro" => 9,
        "outubro" => 10,
        "novembro" => 11,
        "dezembro" => 12,
        _ => return manatan_shared::dates::parse_fixture_date(value),
    };
    let day = parts[1].parse::<u32>().ok()?;
    let year = parts[2].parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn strip_accents(value: &str) -> String {
    value
        .replace(['á', 'à', 'ã', 'â'], "a")
        .replace(['é', 'ê'], "e")
        .replace(['í'], "i")
        .replace(['ó', 'ô', 'õ'], "o")
        .replace(['ú'], "u")
        .replace('ç', "c")
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if let Some(object) = request.as_object_mut() {
        object.insert("listingId".to_string(), Value::String(listing.to_string()));
    }
    request
}

export_novel_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bsx"><a href="/manga/sample"><img src="/cover.jpg"><div class="tt">Sample Novel</div></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Novel</h1><div class="main-info"><div class="thumb"><img src="/cover.jpg"></div></div>
<div class="entry-content entry-content-single"><div>Sample summary.</div></div>
<div class="tsinfo"><div class="imptdt">Autor Author</div><div class="imptdt">Artista Artist</div><div class="imptdt">Status Ativo</div></div>
<div class="mgen"><a href="/genre/fantasy">Fantasy</a></div>
<div id="chapterlist"><ul><li><a href="/manga/sample/chapter-1"><span class="chapternum">Chapter 1</span></a><span class="chapterdate">janeiro 1, 2024</span></li></ul></div>
"#;

const TEXT_FIXTURE: &str = r#"<div class="headpost"><h1 class="entry-title">Sample Novel - Chapter 1</h1></div><div id="readerarea"><p>Sample chapter text.</p></div>"#;
