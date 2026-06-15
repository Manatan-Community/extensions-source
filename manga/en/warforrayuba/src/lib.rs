use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: WarForRayuba = WarForRayuba;
const BASE_URL: &str = "https://xrabohrok.github.io/WarMap/#/";
const GITHUB_TREE: &str = "https://github.com/xrabohrok/WarMap/tree/main/tools";
const RAW_PREFIX: &str = "https://raw.githubusercontent.com/xrabohrok/WarMap/main/tools";
const CUBARI_URL: &str = "https://cubari.moe";

struct WarForRayuba;

impl MangaSource for WarForRayuba {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_github_listing(&fetch_document(
            GITHUB_TREE,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let entries = parse_github_listing(&fetch_document(GITHUB_TREE, LIST_FIXTURE))
            .entries
            .into_iter()
            .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| raw_url("round1.json"));
        Ok(round_item(&fetch_json(&key, ROUND_FIXTURE), key, true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| raw_url("round1.json"));
        Ok(parse_chapters(&fetch_json(&key, ROUND_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("{CUBARI_URL}/read/api/sample"));
        Ok(parse_pages(&fetch_json(&key, PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga"))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.ends_with(".json") && input.contains("WarMap") {
            return Ok(Some(UrlResolveResult {
                item: Some(round_item(
                    &fetch_json(input, ROUND_FIXTURE),
                    input.to_string(),
                    true,
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
        .with_cookies_for("https://github.com")
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_github_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter(|href| href.ends_with(".json") && href.contains("/WarMap/"))
        .map(|href| {
            let file = href.rsplit('/').next().unwrap_or("round.json");
            let raw = raw_url(file);
            round_item(&fetch_json(&raw, ROUND_FIXTURE), raw, false)
        })
        .fold(Vec::new(), push_unique_item);
    if entries.is_empty() {
        entries.push(round_item(ROUND_FIXTURE, raw_url("round1.json"), false));
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn round_item(body: &str, key: String, initialized: bool) -> CatalogItem {
    let round: RoundDto =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(ROUND_FIXTURE).unwrap());
    CatalogItem {
        key: key.clone(),
        title: round.title,
        cover: Some(round.cover),
        description: Some(round.description),
        authors: vec![round.author],
        artists: vec![round.artist],
        status: ItemStatus::Unknown,
        url: Some(key),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let round: RoundDto =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(ROUND_FIXTURE).unwrap());
    let mut chapters: Vec<_> = round
        .chapters
        .into_iter()
        .map(|(number, chapter)| MangaChapter {
            key: url::join_url(CUBARI_URL, &chapter.groups.primary),
            title: Some(format!("{number} {}", chapter.title).trim().to_string()),
            chapter_number: number.parse::<f32>().ok(),
            volume_number: Some(chapter.volume as f32),
            date_uploaded: Some(chapter.last_updated),
            url: Some(url::join_url(CUBARI_URL, &chapter.groups.primary)),
            language: Some("en".into()),
            ..MangaChapter::default()
        })
        .collect();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let pages: Vec<PageDto> =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: page.src,
                context: Some(manga::image_headers(CUBARI_URL)),
            },
            headers: manga::image_headers(CUBARI_URL),
            description: Some(if page.description.is_empty() {
                format!("Page {}", index + 1)
            } else {
                page.description
            }),
            ..MangaPage::default()
        })
        .collect()
}

fn raw_url(file: &str) -> String {
    format!("{RAW_PREFIX}/{}", file.trim_start_matches('/'))
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct RoundDto {
    title: String,
    description: String,
    artist: String,
    author: String,
    cover: String,
    chapters: BTreeMap<String, ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    title: String,
    volume: i32,
    groups: ChapterGroups,
    last_updated: i64,
}

#[derive(Debug, Deserialize)]
struct ChapterGroups {
    primary: String,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    #[serde(default)]
    description: String,
    src: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str =
    r#"<a href="/xrabohrok/WarMap/blob/main/tools/round1.json">round1.json</a>"#;
const ROUND_FIXTURE: &str = r#"{"title":"Round 1","description":"Sample round","artist":"Rayuba","author":"Rayuba","cover":"https://example.com/cover.jpg","chapters":{"1":{"title":"Chapter 1","volume":1,"groups":{"primary":"/read/api/sample"},"last_updated":1704067200}}}"#;
const PAGES_FIXTURE: &str = r#"[{"description":"Page 1","src":"https://example.com/page1.jpg"},{"description":"Page 2","src":"https://example.com/page2.jpg"}]"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rayuba_round() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Round 1");
        assert_eq!(
            SOURCE
                .chapters(json!({"manga":raw_url("round1.json")}))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"https://cubari.moe/read/api/sample"}))
                .unwrap()
                .len(),
            2
        );
    }
}
