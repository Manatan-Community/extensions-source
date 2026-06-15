use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: LightNovelFr = LightNovelFr;
const BASE_URL: &str = "https://lightnovelfr.com";

struct LightNovelFr;

impl NovelSource for LightNovelFr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let body = fetch_document_or_fixture(
            &series_url(page, listing == "latest", &request),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
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
        let page = page(&request);
        let target = format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty() && has_next_page(&body),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/series/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "/series/sample/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        chapters.reverse();
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
            .unwrap_or_else(|| "/series/sample/chapitre-1/".to_string());
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
                has_more: popular.has_next_page,
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

fn series_url(page: u64, latest: bool, request: &Value) -> String {
    let mut parts = vec![format!("page={page}")];
    if latest {
        parts.push("order=latest".to_string());
    }
    for key in ["genre[]", "type[]", "status", "order"] {
        for value in filter_values(request, key) {
            if !value.is_empty() {
                parts.push(format!(
                    "{}={}",
                    url::query_escape(key),
                    url::query_escape(&value)
                ));
            }
        }
    }
    format!("{BASE_URL}/series/?{}", parts.join("&"))
}

fn filter_values(request: &Value, id: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").or(Some(value)))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut seen = BTreeSet::new();
    body.split("<article")
        .skip(1)
        .filter_map(|article| {
            let href = html::attr_after(article, "<a", "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::attr_after(article, "<a", "title")
                .or_else(|| link_text(article))
                .unwrap_or_else(|| title_from_key(&key));
            Some(catalog_item(key, title, image_from(article), false))
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = catalog_item(
        normalize_key(key),
        html::attr_after(body, "ts-post-image", "title")
            .or_else(|| text_between_tag(body, "h1"))
            .unwrap_or_else(|| title_from_key(key)),
        html::attr_after(body, "ts-post-image", "data-src")
            .or_else(|| html::attr_after(body, "ts-post-image", "src"))
            .or_else(|| image_from(body)),
        true,
    );
    item.description = summary(body);
    item.tags = tags(body);
    item.authors = detail_value(body, &["author", "auteur"])
        .into_iter()
        .collect();
    item.status = parse_status(
        &detail_value(body, &["status", "statut"])
            .unwrap_or_else(|| text_after(body, "sertostat").unwrap_or_default()),
    );
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let area = block_after(body, "eplister").unwrap_or_else(|| body.to_string());
    let mut seen = BTreeSet::new();
    area.split("<li")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let number = text_after(block, "epl-num")
                .and_then(|value| chapter_number(&value))
                .or_else(|| chapter_number(&key));
            let title = text_after(block, "epl-title").or_else(|| link_text(block));
            Some(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: number,
                url: Some(absolute_url(&key)),
                language: Some("fr".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let raw = block_after(body, "epcontent")
        .or_else(|| block_after(body, "entry-content"))
        .unwrap_or_else(|| body.to_string());
    let only_paragraphs = raw
        .split("<p")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</p>").map(|value| format!("<p>{value}</p>"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = if only_paragraphs.is_empty() {
        raw
    } else {
        only_paragraphs
    };
    let normalized =
        novel::normalize_reader_html(&remove_blocks(&cleaned, &["code-block", "script"]));
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

fn summary(body: &str) -> Option<String> {
    let block = block_after(body, "entry-content")
        .or_else(|| block_after(body, "itemprop=\"description\""))
        .or_else(|| block_after(body, "itemprop='description'"))?;
    Some(html::strip_tags(&block)).filter(|value| !value.is_empty())
}

fn tags(body: &str) -> Vec<String> {
    let area = block_after(body, "genxed")
        .or_else(|| block_after(body, "sertogenre"))
        .unwrap_or_default();
    area.split("<a").skip(1).filter_map(link_text).collect()
}

fn detail_value(body: &str, labels: &[&str]) -> Option<String> {
    for block in body.split("<span").skip(1) {
        let text = html::text_between(block, ">", "</span>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        let lower = text.to_ascii_lowercase();
        if labels
            .iter()
            .any(|label| lower.trim_end_matches(':').trim() == *label)
        {
            let rest = block
                .split_once("</span>")
                .map(|(_, rest)| rest)
                .unwrap_or_default();
            return html::text_between(rest, ">", "</span>")
                .map(|value| html::strip_tags(&value))
                .or_else(|| {
                    let stripped = html::strip_tags(rest);
                    Some(
                        stripped
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                    )
                })
                .filter(|value| !value.is_empty());
        }
    }
    None
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed")
        || lower.contains("complet")
        || lower.contains("compl\u{e9}t\u{e9}")
    {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("pause") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") || lower.contains("en cours") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn image_from(block: &str) -> Option<String> {
    html::attr_after(block, "<img", "data-src")
        .or_else(|| html::attr_after(block, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_text(chunk: &str) -> Option<String> {
    html::text_between(chunk, "<a", "</a>")
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
        .find("bottomnav")
        .or_else(|| rest.find("</article>"))
        .or_else(|| rest.find("</main>"))
        .or_else(|| rest.find("</section>"))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn remove_blocks(input: &str, markers: &[&str]) -> String {
    input
        .split("<div")
        .enumerate()
        .filter_map(|(idx, part)| {
            let marked = markers.iter().any(|marker| part.contains(marker));
            if idx > 0 && marked {
                None
            } else if idx == 0 {
                Some(part.to_string())
            } else {
                Some(format!("<div{part}"))
            }
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("rel=\"next\"")
        || body.contains("next page-numbers")
        || body.contains("page-numbers next")
}

fn chapter_number(input: &str) -> Option<f32> {
    input
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter(|part| !part.is_empty())
        .next_back()
        .and_then(|part| part.parse().ok())
}

fn title_from_key(key: &str) -> String {
    url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .contains("lightnovelfr.com")
        .then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://lightnovelfr.com/")
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

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const LIST_FIXTURE: &str = r#"
<article><a href="https://lightnovelfr.com/series/sample/" title="Sample Novel"><img src="/cover.jpg"></a></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<article><img class="ts-post-image" title="Sample Novel" src="/cover.jpg"><div class="genxed"><a>Action</a></div><div class="entry-content"><p>Sample summary.</p></div><div class="spe"><span>Auteur</span><span>Sample Author</span><span>Statut</span><span>En cours</span></div><div class="eplister"><ul><li><a href="https://lightnovelfr.com/series/sample/chapitre-1/"><span class="epl-num">1</span><span class="epl-title">Chapitre 1</span></a></li></ul></div></article>
"#;

const TEXT_FIXTURE: &str = r#"
<h1>Chapitre 1</h1><div class="epcontent"><p>Sample chapter text.</p></div><div class="bottomnav"></div>
"#;

export_novel_source!(SOURCE);
