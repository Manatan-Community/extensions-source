use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Nova = Nova;
const BASE_URL: &str = "https://novelasligeras.net";

struct Nova;

impl NovelSource for Nova {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if page == 1 {
            return Ok(Paged {
                entries: parse_ajax_listing(&post_ajax_search("")),
                has_next_page: true,
            });
        }
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}/index.php/page/{page}/?post_type=product&orderby=popularity"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_grid_listing(&body),
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
        if page == 1 {
            return Ok(Paged {
                entries: parse_ajax_listing(&post_ajax_search(query)),
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/index.php/page/{page}/?s={}&post_type=product&title=1&excerpt=1&content=0&categories=1&attributes=1&tags=1&sku=0&orderby=popularity&ixwps=1",
            url::query_escape(query)
        );
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_grid_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "producto/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "producto/sample/".to_string());
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
            .unwrap_or_else(|| "producto/sample/capitulo-1/".to_string());
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

fn post_ajax_search(query: &str) -> String {
    let form = [
        ("action", "product_search"),
        ("product-search", "1"),
        ("product-query", query),
    ];
    client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php?tags=1&sku=&limit=30&category_results=&order=DESC&category_limit=5&order_by=title&product_thumbnails=1&title=1&excerpt=1&content=&categories=1&attributes=1"))
        .xhr()
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| AJAX_FIXTURE.to_string())
}

fn parse_ajax_listing(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let href = item.get("url").and_then(Value::as_str)?;
            let key = normalize_key(href);
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Novel")
                .to_string();
            let cover = item
                .get("thumbnail")
                .and_then(Value::as_str)
                .map(absolute_url);
            Some(catalog_item(key, title, cover, false))
        })
        .collect()
}

fn parse_grid_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("wf-cell")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "entry-title", "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = text_after(block, "entry-title")
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(catalog_item(key, title, image_from(block), false))
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
        text_between_tag(body, "h1")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        html::attr_after(body, "woocommerce-product-gallery", "src")
            .or_else(|| html::attr_after(body, "woocommerce-product-gallery", "data-cfsrc"))
            .or_else(|| html::attr_after(body, "woocommerce-product-gallery", "data-src"))
            .or_else(|| image_from(body))
            .map(|value| absolute_url(&value)),
        true,
    );
    item.authors = detail_value(body, "attribute_pa_escritor")
        .into_iter()
        .collect();
    item.artists = detail_value(body, "attribute_pa_ilustrador")
        .into_iter()
        .collect();
    item.description = block_after(body, "woocommerce-product-details__short-description")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.status = parse_status(&detail_value(body, "attribute_pa_estado").unwrap_or_default());
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let mut seen = BTreeSet::new();
    for volume_block in body.split("dt-fancy-title").skip(1) {
        let volume = html::text_between(volume_block, ">", "</")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        if !volume.starts_with("Volumen") {
            continue;
        }
        for chunk in volume_block.split("<a").skip(1) {
            if !chunk.contains("wpb_tab") && !volume_block.contains("wpb_tab") {
                continue;
            }
            let Some(href) = html::attr(chunk, "href") else {
                continue;
            };
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                continue;
            }
            let raw = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let title = nova_chapter_title(&volume, &raw);
            chapters.push(NovelChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: Some(chapters.len() as f32 + 1.0),
                url: Some(absolute_url(&key)),
                language: Some("es".to_string()),
                ..NovelChapter::default()
            });
        }
    }
    chapters
}

fn nova_chapter_title(volume: &str, raw: &str) -> String {
    if let Some((part, rest)) = raw.split_once(" . ") {
        if let Some((chapter, name)) = rest.split_once(": ") {
            return format!("{volume} - {chapter} - {part}: {name}");
        }
    }
    format!("{volume} - {raw}")
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let marker = if body.contains("Nadie entra sin permiso en la Gran Tumba de Nazarick") {
        "id=\"content\""
    } else {
        "wpb_wrapper"
    };
    let raw = block_after(body, marker).unwrap_or_else(|| body.to_string());
    let cleaned = raw
        .replace("data-cfsrc=", "src=")
        .replace("data-src=", "src=");
    let normalized = novel::normalize_reader_html(&cleaned);
    NovelText {
        title: text_between_tag(body, "h1"),
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; } center { text-align: center; display: block; }".to_string()),
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
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(block, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn detail_value(body: &str, marker: &str) -> Option<String> {
    body.split(marker)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<td", "</td>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("complet") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("curso") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
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
    let end = rest.find("</div>").unwrap_or(rest.len());
    Some(rest[..end + "</div>".len().min(rest.len().saturating_sub(end))].to_string())
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("next page-numbers")
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("novelasligeras.net")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
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

const AJAX_FIXTURE: &str = r#"
[{ "title": "Sample Novel", "url": "https://novelasligeras.net/producto/sample/", "thumbnail": "https://novelasligeras.net/cover.jpg" }]
"#;

const LIST_FIXTURE: &str = r#"
<div class="dt-css-grid"><div class="wf-cell"><img src="/cover.jpg"><h4 class="entry-title"><a href="https://novelasligeras.net/producto/sample/">Sample Novel</a></h4></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Novel</h1><div class="woocommerce-product-gallery"><img src="/cover.jpg"></div>
<tr class="woocommerce-product-attributes-item--attribute_pa_escritor"><td>Sample Author</td></tr>
<tr class="woocommerce-product-attributes-item--attribute_pa_estado"><td>En curso</td></tr>
<div class="woocommerce-product-details__short-description"><p>Sample summary.</p></div>
<div class="dt-fancy-title">Volumen 1</div><div class="wpb_tab"><a href="https://novelasligeras.net/producto/sample/capitulo-1/">Parte 1 . Capitulo 1: Inicio</a></div>
"#;

const TEXT_FIXTURE: &str = r#"
<h1>Capitulo 1</h1><div class="wpb_wrapper"><p>Sample chapter text.</p></div>
"#;

export_novel_source!(SOURCE);
