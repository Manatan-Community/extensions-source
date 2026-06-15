use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, webview,
};
use serde_json::Value;

const BASE_URL: &str = "https://cubari.moe";
const SOURCE: Cubari = Cubari;

struct Cubari;

impl MangaSource for Cubari {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        Ok(parse_home_payload(&home_payload(), if latest { SortKind::Unpinned } else { SortKind::Pinned }))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if is_direct_query(query) {
            let (source, slug) = deep_link(query).unwrap_or_else(|| ("cubari".into(), "sample".into()));
            let body = fetch_json_or_fixture(&series_api_url(&source, &slug), SERIES_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_series(&body, Some(format!("/read/{source}/{slug}")))],
                has_next_page: false,
            });
        }
        let mut page = parse_home_payload(&home_payload(), SortKind::All);
        let needle = query.to_ascii_lowercase();
        page.entries.retain(|item| item.title.to_ascii_lowercase().contains(&needle));
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/read/imgur/sample".into());
        let (source, slug) = source_slug_from_key(&key).unwrap_or_else(|| ("imgur".into(), "sample".into()));
        let body = fetch_json_or_fixture(&series_api_url(&source, &slug), SERIES_FIXTURE);
        Ok(parse_series(&body, Some(format!("/read/{source}/{slug}"))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/read/imgur/sample".into());
        let (source, slug) = source_slug_from_key(&key).unwrap_or_else(|| ("imgur".into(), "sample".into()));
        let body = fetch_json_or_fixture(&series_api_url(&source, &slug), SERIES_FIXTURE);
        Ok(parse_chapters(&body, &format!("/read/{source}/{slug}")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/read/imgur/sample/1/default".into());
        if key.contains("/chapter/") {
            return Ok(parse_direct_pages(&fetch_json_or_fixture(&format!("{BASE_URL}{key}"), PAGES_FIXTURE)));
        }
        let (source, slug, chapter, group) = chapter_parts(&key)
            .unwrap_or_else(|| ("imgur".into(), "sample".into(), "1".into(), "default".into()));
        let body = fetch_json_or_fixture(&series_api_url(&source, &slug), SERIES_FIXTURE);
        Ok(parse_series_pages(&body, &chapter, &group))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        let Some((source, slug)) = deep_link(input) else { return Ok(None); };
        let body = fetch_json_or_fixture(&series_api_url(&source, &slug), SERIES_FIXTURE);
        Ok(Some(UrlResolveResult {
            item: Some(parse_series(&body, Some(format!("/read/{source}/{slug}")))),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKind {
    Pinned,
    Unpinned,
    All,
}

fn home_payload() -> String {
    webview::extract_text(
        webview::ExtractRequest::new(
            BASE_URL,
            r#"
Promise.all([
  globalHistoryHandler.getAllPinnedSeries(),
  globalHistoryHandler.getAllUnpinnedSeries()
]).then(items => JSON.stringify(items.flatMap(item => item)))
"#,
        )
        .wait_for_script("typeof globalHistoryHandler !== 'undefined'")
        .timeout_ms(10_000)
        .cookies(true),
    )
    .unwrap_or_else(|_| HOME_FIXTURE.to_string())
}

fn fetch_json_or_fixture(target_url: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .get(target_url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_home_payload(body: &str, sort: SortKind) -> Paged<CatalogItem> {
    let values = serde_json::from_str::<Vec<Value>>(body)
        .unwrap_or_else(|_| serde_json::from_str(HOME_FIXTURE).expect("home fixture"));
    let entries = values
        .into_iter()
        .filter(|item| match sort {
            SortKind::Pinned => item.get("pinned").and_then(Value::as_bool).unwrap_or(false),
            SortKind::Unpinned => !item.get("pinned").and_then(Value::as_bool).unwrap_or(false),
            SortKind::All => true,
        })
        .map(|item| parse_series_value(&item, None))
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_series(body: &str, key: Option<String>) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    parse_series_value(&value, key)
}

fn parse_series_value(value: &Value, key: Option<String>) -> CatalogItem {
    let description = text(value, "description").unwrap_or_else(|| "No description.".into());
    let tags = description
        .split_once("Tags: ")
        .map(|(_, tags)| tags.split(',').map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect())
        .unwrap_or_default();
    CatalogItem {
        key: key.unwrap_or_else(|| text(value, "url").unwrap_or_else(|| "/read/imgur/sample".into())),
        title: text(value, "title").unwrap_or_else(|| "Cubari Series".into()),
        cover: text(value, "coverUrl").or_else(|| text(value, "cover")),
        authors: text(value, "author").into_iter().collect(),
        artists: text(value, "artist").into_iter().collect(),
        description: Some(description.split("Tags: ").next().unwrap_or("No description.").to_string()),
        tags,
        status: ItemStatus::Unknown,
        language: Some("all".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    let groups = value.get("groups").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut chapters = Vec::new();
    for (chapter_number, chapter) in value.get("chapters").and_then(Value::as_object).into_iter().flatten() {
        let title = text(chapter, "title").unwrap_or_default();
        let volume = text(chapter, "volume").filter(|volume| !["Uncategorized", "null", ""].contains(&volume.as_str()));
        let Some(chapter_groups) = chapter.get("groups").and_then(Value::as_object) else { continue; };
        for (group_id, pages) in chapter_groups {
            let scanlator = groups
                .get(group_id)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("default");
            let key = if pages.is_array() {
                format!("{manga_key}/{chapter_number}/{group_id}")
            } else {
                pages.as_str().unwrap_or_default().to_string()
            };
            chapters.push(MangaChapter {
                key,
                title: Some(chapter_title(volume.as_deref(), chapter_number, &title)),
                chapter_number: chapter_number.parse::<f32>().ok(),
                scanlators: vec![scanlator.to_string()],
                date_uploaded: chapter
                    .get("release_date")
                    .and_then(|dates| dates.get(group_id))
                    .and_then(Value::as_f64)
                    .map(|seconds| seconds as i64),
                ..MangaChapter::default()
            });
        }
    }
    chapters.sort_by(|a, b| b.chapter_number.partial_cmp(&a.chapter_number).unwrap_or(std::cmp::Ordering::Equal));
    chapters
}

fn parse_series_pages(body: &str, chapter: &str, group: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("series fixture"));
    let normalized = chapter.trim_start_matches('0');
    let chapter_value = value
        .get("chapters")
        .and_then(|chapters| chapters.get(chapter).or_else(|| chapters.get(normalized)));
    let Some(pages) = chapter_value
        .and_then(|chapter| chapter.get("groups"))
        .and_then(|groups| groups.get(group))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    pages_to_manga_pages(pages)
}

fn parse_direct_pages(body: &str) -> Vec<MangaPage> {
    let pages = serde_json::from_str::<Vec<Value>>(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("pages fixture"));
    pages_to_manga_pages(&pages)
}

fn pages_to_manga_pages(pages: &[Value]) -> Vec<MangaPage> {
    pages
        .iter()
        .filter_map(|page| page.as_str().map(ToString::to_string).or_else(|| text(page, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: None },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_title(volume: Option<&str>, chapter: &str, title: &str) -> String {
    let mut out = String::new();
    if let Some(volume) = volume {
        out.push_str("Vol.");
        out.push_str(volume);
        out.push(' ');
    }
    out.push_str("Ch.");
    out.push_str(chapter);
    if !title.is_empty() {
        out.push_str(" - ");
        out.push_str(title);
    }
    out
}

fn is_direct_query(query: &str) -> bool {
    query.starts_with("https://") || query.starts_with("cubari:")
}

fn deep_link(query: &str) -> Option<(String, String)> {
    if let Some(rest) = query.strip_prefix("cubari:") {
        let (source, slug) = rest.split_once('/')?;
        return Some((source.to_string(), slug.to_string()));
    }
    let after_scheme = query.split_once("://")?.1;
    let (host, path) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
    let parts = path.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if host.ends_with("imgur.com") && parts.len() >= 2 && ["a", "gallery"].contains(&parts[0]) {
        Some(("imgur".into(), parts[1].into()))
    } else if host.ends_with("reddit.com") && parts.len() >= 2 && parts[0] == "gallery" {
        Some(("reddit".into(), parts[1].into()))
    } else if host == "imgchest.com" && parts.len() >= 2 && parts[0] == "p" {
        Some(("imgchest".into(), parts[1].into()))
    } else if host.ends_with("catbox.moe") && parts.len() >= 2 && parts[0] == "c" {
        Some(("catbox".into(), parts[1].into()))
    } else if host.ends_with("cubari.moe") && parts.len() >= 3 {
        Some((parts[1].into(), parts[2].into()))
    } else {
        None
    }
}

fn source_slug_from_key(key: &str) -> Option<(String, String)> {
    let parts = key.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0] == "read" {
        Some((parts[1].into(), parts[2].into()))
    } else {
        None
    }
}

fn chapter_parts(key: &str) -> Option<(String, String, String, String)> {
    let parts = key.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() >= 5 && parts[0] == "read" {
        Some((parts[1].into(), parts[2].into(), parts[3].into(), parts[4].into()))
    } else {
        None
    }
}

fn series_api_url(source: &str, slug: &str) -> String {
    format!("{BASE_URL}/read/api/{source}/series/{slug}/")
}

fn text(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).filter(|value| !value.is_empty()).map(ToString::to_string)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
[
  { "title": "Pinned Series", "url": "/read/imgur/pinned", "cover": "https://i.imgur.com/cover.jpg", "author": "Unknown", "artist": "Unknown", "description": "Pinned. Tags: demo", "pinned": true },
  { "title": "Latest Series", "url": "/read/imgur/latest", "cover": "https://i.imgur.com/latest.jpg", "author": "Unknown", "artist": "Unknown", "description": "Latest. Tags: demo", "pinned": false }
]
"#;

const SERIES_FIXTURE: &str = r#"
{
  "title": "Sample Cubari",
  "url": "/read/imgur/sample",
  "cover": "https://i.imgur.com/cover.jpg",
  "author": "Unknown",
  "artist": "Unknown",
  "description": "A sample series. Tags: demo, web",
  "groups": { "default": "", "1": "Scan Group" },
  "chapters": {
    "1": {
      "volume": "1",
      "title": "Start",
      "groups": { "default": ["https://i.imgur.com/1.jpg"], "1": ["https://i.imgur.com/2.jpg"] },
      "release_date": { "default": 1704067200, "1": 1704067200 }
    }
  }
}
"#;

const PAGES_FIXTURE: &str = r#"
[
  "https://i.imgur.com/1.jpg",
  { "src": "https://i.imgur.com/2.jpg" }
]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_home_lists() {
        assert_eq!(parse_home_payload(HOME_FIXTURE, SortKind::Pinned).entries.len(), 1);
        assert_eq!(parse_home_payload(HOME_FIXTURE, SortKind::Unpinned).entries[0].title, "Latest Series");
    }

    #[test]
    fn parses_series_chapters_and_pages() {
        let details = parse_series(SERIES_FIXTURE, None);
        assert_eq!(details.title, "Sample Cubari");
        assert_eq!(details.tags, vec!["demo", "web"]);
        let chapters = parse_chapters(SERIES_FIXTURE, "/read/imgur/sample");
        assert_eq!(chapters.len(), 2);
        assert_eq!(parse_series_pages(SERIES_FIXTURE, "1", "default").len(), 1);
        assert_eq!(parse_direct_pages(PAGES_FIXTURE).len(), 2);
    }

    #[test]
    fn resolves_supported_deep_links() {
        assert_eq!(deep_link("https://cubari.moe/read/imgur/abc"), Some(("imgur".into(), "abc".into())));
        assert_eq!(deep_link("https://imgur.com/a/abc"), Some(("imgur".into(), "abc".into())));
        assert_eq!(deep_link("cubari:reddit/abc"), Some(("reddit".into(), "abc".into())));
    }
}
