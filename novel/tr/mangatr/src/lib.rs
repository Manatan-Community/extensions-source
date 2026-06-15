use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangaTr = MangaTr;
const BASE_URL: &str = "https://manga-tr.com";

struct MangaTr;

impl NovelSource for MangaTr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str);
        let target = if listing == Some("latest") {
            format!(
                "{BASE_URL}/manga-list-sayfala.html?page={page}&sort=last_update&sort_type=DESC"
            )
        } else {
            format!(
                "{BASE_URL}/manga-list-sayfala.html?page={page}&durum={}&ceviri=&yas={}&icerik=2&tur={}&sort={}&sort_type={}",
                esc(&lnreader::filter_string(&request, "status", "")),
                esc(&lnreader::filter_string(&request, "age", "")),
                esc(&lnreader::filter_string(&request, "genre", "")),
                esc(&lnreader::filter_string(&request, "sort", "puan")),
                esc(&lnreader::filter_string(&request, "sort_type", "DESC")),
            )
        };
        let body = fetch(&target, LIST_FIXTURE);
        let entries = parse_cards(&body);
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
        if page > 1 {
            return Ok(Paged::default());
        }
        let body = fetch(
            &format!("{BASE_URL}/arama.html?icerik={}", esc(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_search(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "manga-ornek.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "manga-ornek.html".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&key, &body))
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
            .unwrap_or_else(|| "ornek-bolum.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        let raw = extract_id_html(&body, "well").unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(text_response(&key, &raw))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Novels", popular),
            section("latest", "Latest", latest),
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

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn post_chapter_page(title: &str, page: u64) -> String {
    let target = format!(
        "{BASE_URL}/cek/fetch_pages_manga.php?manga_cek={}",
        esc(title)
    );
    lnreader::client(BASE_URL)
        .post(&target)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("x-requested-with", "XMLHttpRequest")
        .body(format!("page={page}").into_bytes())
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string())
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("col-md-12")
        .filter_map(|block| {
            let title = text_between(block, "id=\"tables\"", "</")
                .or_else(|| text_between(block, "<h3", "</h3>"))?;
            let href = attr_after(block, "id=\"tables\"", "href")
                .or_else(|| attr_after(block, "<a", "href"))?;
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: attr_after(block, "img-thumb", "src")
                    .or_else(|| attr_after(block, "<img", "src"))
                    .map(|v| absolute_url(&v)),
                url: Some(absolute_url(&href)),
                language: Some("tr".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .filter(|block| block.contains("manga-slug") || block.contains(".html"))
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            let name = html::strip_tags(block);
            if name.is_empty() || !href.contains(".html") {
                return None;
            }
            Some(CatalogItem {
                key: normalize_key(&href),
                title: name,
                url: Some(absolute_url(&href)),
                language: Some("tr".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    parse_details(&fetch(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "id=\"tables\"", "</")
            .or_else(|| text_between(body, "<h1", "</h1>"))
            .unwrap_or_else(|| "MangaTR".to_string()),
        cover: attr_after(body, "<img", "src").map(|v| absolute_url(&v)),
        description: html::text_between(body, "class=\"well\"", "</div>")
            .map(|v| html::strip_tags(&v)),
        authors: collect_link_texts(body, "Yazar").into_iter().collect(),
        artists: collect_link_texts(body, "Çizer"),
        tags: collect_link_texts(body, "Tür"),
        status: status_from_text(body),
        url: Some(absolute_url(key)),
        language: Some("tr".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(key: &str, body: &str) -> Vec<NovelChapter> {
    let slug = key.trim_start_matches("manga-").trim_end_matches(".html");
    let first = post_chapter_page(slug, 1);
    let last = attr_after(&first, "title=\"Last\"", "data-page")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1)
        .min(25);
    let mut all = first;
    for page in 2..=last {
        all.push_str(&post_chapter_page(slug, page));
    }
    let source = if all.trim().is_empty() { body } else { &all };
    let mut chapters = source
        .split("<tr")
        .enumerate()
        .filter_map(|(index, row)| {
            let href = attr_after(row, "<a", "href")?;
            let title =
                text_between(row, "<a", "</a>").unwrap_or_else(|| format!("Ch {}", index + 1));
            Some(NovelChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: extract_number(row).or(Some((index + 1) as f32)),
                url: Some(absolute_url(&href)),
                language: Some("tr".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn text_response(key: &str, raw: &str) -> NovelText {
    let normalized = novel::normalize_reader_html(raw);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn collect_link_texts(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|idx| {
            body[idx..]
                .split("<a")
                .skip(1)
                .take(8)
                .filter_map(|part| text_between(part, ">", "</a>"))
                .collect()
        })
        .unwrap_or_default()
}

fn status_from_text(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("tamam") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("devam") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn extract_number(input: &str) -> Option<f32> {
    let digits = html::strip_tags(input)
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    digits.parse().ok()
}

fn esc(value: &str) -> String {
    url::query_escape(value)
}

fn absolute_url(key: &str) -> String {
    lnreader::absolute_url(BASE_URL, key)
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    lnreader::key_from_url(BASE_URL, input)
}

fn attr_after(input: &str, marker: &str, attr: &str) -> Option<String> {
    html::attr_after(input, marker, attr).filter(|value| !value.trim().is_empty())
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    if start == ">" {
        let idx = input.find('>')?;
        let rest = &input[idx + 1..];
        let end_idx = rest.find(end)?;
        return Some(html::strip_tags(&rest[..end_idx]));
    }
    html::text_between(input, start, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn extract_id_html(input: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{id}\"");
    html::text_between(input, &marker, "</div>").filter(|value| !value.trim().is_empty())
}

const LIST_FIXTURE: &str = r#"<div class="col-md-12"><h3 id="tables"><a href="/manga-ornek.html">Ornek Novel</a></h3><img class="img-thumb" src="/cover.jpg"></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="char"><a href="/manga-ornek.html" manga-slug="ornek">Ornek Novel</a><span>Novel</span></div>"#;
const DETAILS_FIXTURE: &str =
    r#"<h1 id="tables">Ornek Novel</h1><div class="well"><p>Summary.</p></div>"#;
const CHAPTERS_FIXTURE: &str = r#"<body><ul><table><tr><td><a href="/ornek-bolum.html">Ornek Ch 1</a></td></tr></table></ul></body>"#;
const TEXT_FIXTURE: &str = r#"<div id="well"><p>Ornek metin.</p></div>"#;

export_novel_source!(SOURCE);
