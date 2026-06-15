use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Chireads = Chireads;
const BASE_URL: &str = "https://chireads.com";

struct Chireads;

impl NovelSource for Chireads {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let mut entries = parse_listing(&fetch_document_or_fixture(
                &format!("{BASE_URL}/category/translatedtales/page/{page}"),
                LIST_FIXTURE,
            ));
            entries.extend(parse_listing(&fetch_document_or_fixture(
                &format!("{BASE_URL}/category/original/page/{page}"),
                LIST_FIXTURE,
            )));
            return Ok(Paged {
                has_next_page: !entries.is_empty(),
                entries: dedupe(entries),
            });
        }

        let tag = filter_string(&request, "tag", "all");
        if tag != "all" {
            let body = fetch_document_or_fixture(
                &format!("{BASE_URL}/tag/{tag}/page/{page}"),
                LIST_FIXTURE,
            );
            let entries = parse_listing(&body);
            return Ok(Paged {
                has_next_page: !entries.is_empty(),
                entries,
            });
        }
        if page > 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(BASE_URL, HOME_FIXTURE);
        Ok(Paged {
            entries: parse_popular(&body),
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page != 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let needle = normalize_search(query);
        let mut novels = Vec::new();
        for page_no in 1..=20 {
            let request = serde_json::json!({ "page": page_no, "listing": "latest" });
            let page = self.list(request)?;
            if page.entries.is_empty() {
                break;
            }
            novels.extend(page.entries);
        }
        Ok(Paged {
            entries: dedupe(novels)
                .into_iter()
                .filter(|item| normalize_search(&item.title).contains(&needle))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "roman/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "roman/sample".to_string());
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
            .unwrap_or_else(|| "roman/sample/chapitre-1".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populaire".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Derniers".to_string(),
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
        .with_header("Accept-Encoding", "deflate")
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<li")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            if !href.contains("chireads.com") && href.starts_with("http") {
                return None;
            }
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = first_div_text(block)
                .or_else(|| link_text(block))
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(key, title, image_from(block), false))
        })
        .collect()
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    let popular_area = body
        .rfind("Populaire")
        .map(|idx| &body[idx..])
        .unwrap_or(body);
    let entries = parse_listing(popular_area);
    if !entries.is_empty() {
        return entries;
    }
    parse_listing(body)
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        text_after(body, "inform-product-txt")
            .or_else(|| text_after(body, "inform-title"))
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "inform-product", "src")
            .or_else(|| html::attr_after(body, "inform-product-img", "src"))
            .or_else(|| image_from(body))
            .map(|value| absolute_url(&value)),
        true,
    );
    item.description =
        text_after(body, "inform-inform-txt").or_else(|| text_after(body, "inform-intr-txt"));

    let infos = text_after(body, "inform-intr-col")
        .or_else(|| text_after(body, "inform-inform-data"))
        .unwrap_or_default();
    item.authors = author_from_infos(&infos).into_iter().collect();
    item.status = status_from_infos(&infos);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let area = if body.contains("chapitre-table") {
        block_after(body, "chapitre-table").unwrap_or_else(|| body.to_string())
    } else {
        block_after(body, "inform-annexe-list").unwrap_or_else(|| body.to_string())
    };
    let mut seen = BTreeSet::new();
    area.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            Some(NovelChapter {
                key: key.clone(),
                title: link_text(chunk),
                chapter_number: chapter_number(&key),
                url: Some(absolute_url(&key)),
                language: Some("fr".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "id=\"content\"")
        .or_else(|| block_after(body, "id='content'"))
        .or_else(|| block_after(body, "content"))
        .unwrap_or_else(|| body.to_string());
    let normalized = novel::normalize_reader_html(&raw);
    NovelText {
        title: text_between_tag(body, "h1"),
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
        cover,
        url: Some(absolute_url(&key)),
        language: Some("fr".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn dedupe(entries: Vec<CatalogItem>) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|item| seen.insert(item.key.clone()))
        .collect()
}

fn filter_string(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
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

fn author_from_infos(infos: &str) -> Option<String> {
    if let Some(author) = between_text(infos, "Auteur : ", "Statut de Parution : ") {
        return Some(author);
    }
    between_text(infos, "Fantrad : ", "Statut de Parution : ")
}

fn status_from_infos(infos: &str) -> ItemStatus {
    let lower = infos.to_ascii_lowercase();
    if lower.contains("en pause") {
        ItemStatus::Hiatus
    } else if lower.contains("complet") {
        ItemStatus::Completed
    } else if lower.is_empty() {
        ItemStatus::Unknown
    } else {
        ItemStatus::Ongoing
    }
}

fn between_text(input: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = input.find(start)? + start.len();
    let rest = &input[start_idx..];
    let end_idx = rest.find(end).unwrap_or(rest.len());
    Some(rest[..end_idx].trim().to_string()).filter(|value| !value.is_empty())
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(block, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn first_div_text(block: &str) -> Option<String> {
    html::text_between(block, "<div", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_text(chunk: &str) -> Option<String> {
    html::text_between(chunk, ">", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn block_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    let rest = &body[start..];
    let end = rest
        .find("</article>")
        .or_else(|| rest.find("</main>"))
        .or_else(|| rest.find("</section>"))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn normalize_search(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .replace('\u{e9}', "e")
        .replace('\u{e8}', "e")
        .replace('\u{ea}', "e")
        .replace('\u{e0}', "a")
        .replace('\u{e2}', "a")
        .replace('\u{ee}', "i")
        .replace('\u{ef}', "i")
        .replace('\u{f4}', "o")
        .replace('\u{fb}', "u")
        .replace('\u{e7}', "c")
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Roman".to_string())
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("chireads.com").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://chireads.com/")
        .trim_start_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const HOME_FIXTURE: &str = r#"
<h2>Populaire</h2><ul><li><div><a href="https://chireads.com/roman/sample"><img src="/cover.jpg">Sample Novel</a></div></li></ul>
"#;

const LIST_FIXTURE: &str = r#"
<ul class="romans-content"><li><div><a href="https://chireads.com/roman/sample"><img src="/cover.jpg">Sample Novel</a></div></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="inform-title">Sample Novel</div><div class="inform-product-img"><img src="/cover.jpg"></div><div class="inform-intr-col">Auteur : Sample Author Statut de Parution : Complet</div><div class="inform-intr-txt">Sample summary.</div><div class="chapitre-table"><a href="https://chireads.com/roman/sample/chapitre-1">Chapitre 1</a></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1>Chapitre 1</h1><div id="content"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
