use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: EpikNovel = EpikNovel;
const BASE_URL: &str = "https://www.epiknovel.com";

struct EpikNovel;

impl NovelSource for EpikNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch(
            &format!("{BASE_URL}/seri-listesi?Sayfa={page}"),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
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
        let body = fetch(
            &format!(
                "{BASE_URL}/seri-listesi?q={}&Sayfa={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "ornek-seri".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "ornek-seri".to_string());
        Ok(parse_chapters(&fetch(&absolute_url(&key), DETAILS_FIXTURE)))
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "ornek-bolum".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        let raw = extract_id_html(&body, "icerik").unwrap_or_else(|| "Premium Chapter".to_string());
        Ok(text_response(&key, &raw))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Seriler".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
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

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn absolute_url(key: &str) -> String {
    lnreader::absolute_url(BASE_URL, key)
}

fn key_from_url(input: &str) -> Option<String> {
    lnreader::key_from_url(BASE_URL, input)
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("col-lg-12 col-md-12")
        .filter_map(|block| {
            let href =
                attr_after(block, "<h3", "href").or_else(|| attr_after(block, "<a", "href"))?;
            let title =
                text_between(block, "<h3", "</h3>").unwrap_or_else(|| "EpikNovel".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: attr_after(block, "<img", "data-src")
                    .or_else(|| attr_after(block, "<img", "src"))
                    .map(|v| absolute_url(&v)),
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
        title: text_between(body, "<h1", "</h1>")
            .or_else(|| text_between(body, "id=\"tables\"", "</"))
            .unwrap_or_else(|| "EpikNovel".to_string()),
        cover: attr_after(body, "manga-cover", "src").map(|v| absolute_url(&v)),
        description: text_between(body, "<p", "</p>"),
        authors: text_after(body, "Publisher:").into_iter().collect(),
        status: status_from_text(&body.to_ascii_lowercase()),
        url: Some(absolute_url(key)),
        language: Some("tr".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("<tr")
        .enumerate()
        .filter_map(|(index, row)| {
            let href = attr_after(row, "<a", "href")?;
            let title =
                text_between(row, "<a", "</a>").unwrap_or_else(|| format!("Bölüm {}", index));
            Some(NovelChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: Some(index as f32),
                url: Some(absolute_url(&href)),
                language: Some("tr".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
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

fn status_from_text(text: &str) -> ItemStatus {
    if text.contains("tamam") || text.contains("completed") {
        ItemStatus::Completed
    } else if text.contains("devam") || text.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn attr_after(input: &str, marker: &str, attr: &str) -> Option<String> {
    html::attr_after(input, marker, attr).filter(|value| !value.trim().is_empty())
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    html::text_between(input, start, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn extract_id_html(input: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{id}\"");
    html::text_between(input, &marker, "</div>").filter(|value| !value.trim().is_empty())
}

fn text_after(input: &str, marker: &str) -> Option<String> {
    input
        .find(marker)
        .map(|idx| {
            input[idx + marker.len()..]
                .lines()
                .next()
                .map(html::strip_tags)
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

const LIST_FIXTURE: &str = r#"<div class="col-lg-12 col-md-12"><h3><a href="/ornek-seri">Örnek Seri</a></h3><img data-src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 id="tables">Örnek Seri</h1><img class="manga-cover" src="/cover.jpg"><table><tr><td><a href="/ornek-bolum">Bölüm 1</a></td></tr></table>"#;
const TEXT_FIXTURE: &str = r#"<div id="icerik"><p>Örnek bölüm metni.</p></div>"#;

export_novel_source!(SOURCE);
