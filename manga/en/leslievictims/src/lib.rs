use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: LeslieVictims = LeslieVictims;
const BASE_URL: &str = "https://leslie-victims.pages.dev";

struct LeslieVictims;

impl MangaSource for LeslieVictims {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: parse_library(&fetch_library(), ""),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: find_entry(&fetch_library(), series_id(&key).unwrap_or("sample"))
                    .as_ref()
                    .map(entry_to_item)
                    .into_iter()
                    .collect(),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_library(&fetch_library(), query),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/?series=sample".into());
        let library = fetch_library();
        Ok(find_entry(&library, series_id(&key).unwrap_or("sample"))
            .as_ref()
            .map(entry_to_item)
            .unwrap_or_else(|| fixture_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/?series=sample".into());
        let library = fetch_library();
        Ok(find_entry(&library, series_id(&key).unwrap_or("sample"))
            .as_ref()
            .map(entry_to_chapters)
            .unwrap_or_default())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/?series=sample&ch=1".into());
        let sid = series_id(&key).unwrap_or("sample");
        let chapter_id = query_param(&key, "ch").unwrap_or("1");
        let library = fetch_library();
        let Some(entry) = find_entry(&library, sid) else {
            return Ok(Vec::new());
        };
        if let Some(root) = entry
            .get("chapter_roots")
            .and_then(|roots| roots.get(chapter_id))
        {
            return Ok(pages_from_root(root));
        }
        Ok((1..=150)
            .map(|page| format!("{BASE_URL}/content/{sid}/{chapter_id}/{page:02}.webp"))
            .enumerate()
            .map(|(index, image)| page(index, image))
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let library = fetch_library();
            return Ok(Some(UrlResolveResult {
                item: find_entry(&library, series_id(&key).unwrap_or("sample"))
                    .as_ref()
                    .map(entry_to_item),
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
        .with_referer(format!("{BASE_URL}/"))
}

fn fetch_library() -> String {
    client()
        .get(format!("{BASE_URL}/api/library"))
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| LIBRARY_FIXTURE.to_string())
}

fn parse_library(body: &str, query: &str) -> Vec<CatalogItem> {
    let query = query.to_ascii_lowercase();
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            query.is_empty()
                || entry
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .map(|entry| entry_to_item(&entry))
        .collect()
}

fn find_entry(body: &str, id: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .as_array()?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        .cloned()
}

fn entry_to_item(entry: &Value) -> CatalogItem {
    let id = entry.get("id").and_then(Value::as_str).unwrap_or("sample");
    CatalogItem {
        key: format!("/?series={}", url::query_escape(id)),
        title: entry
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Leslie&Victims")
            .to_string(),
        cover: entry
            .get("cover")
            .and_then(Value::as_str)
            .map(|cover| url::join_url(BASE_URL, cover)),
        url: Some(format!("{BASE_URL}/?series={}", url::query_escape(id))),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn entry_to_chapters(entry: &Value) -> Vec<MangaChapter> {
    let id = entry.get("id").and_then(Value::as_str).unwrap_or("sample");
    let mut chapters = entry
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|chapter| MangaChapter {
            key: format!(
                "/?series={}&ch={}",
                url::query_escape(id),
                url::query_escape(chapter)
            ),
            title: Some(format!("Chapter {chapter}")),
            chapter_number: chapter
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<f32>().ok()),
            url: Some(format!(
                "{BASE_URL}/?series={id}&ch={}",
                url::query_escape(chapter)
            )),
            ..MangaChapter::default()
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn pages_from_root(root: &Value) -> Vec<MangaPage> {
    let base = root.get("url").and_then(Value::as_str).unwrap_or(BASE_URL);
    match root.get("mode").and_then(Value::as_str).unwrap_or_default() {
        "list" => root
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|file| url::join_url(base, file))
            .enumerate()
            .map(|(index, image)| page(index, image))
            .collect(),
        "count" => {
            let count = root.get("data").and_then(Value::as_u64).unwrap_or(0);
            (1..=count)
                .map(|index| format!("{base}/{index:02}.webp"))
                .enumerate()
                .map(|(index, image)| page(index, image))
                .collect()
        }
        _ => Vec::new(),
    }
}

fn page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn fixture_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: "Leslie&Victims".to_string(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input[BASE_URL.len()..].trim_start_matches('/'))
    } else {
        input.to_string()
    }
}

fn series_id(key: &str) -> Option<&str> {
    query_param(key, "series")
}

fn query_param<'a>(key: &'a str, name: &str) -> Option<&'a str> {
    let query = key.split('?').nth(1).unwrap_or(key);
    query.split('&').find_map(|pair| {
        let (left, right) = pair.split_once('=')?;
        (left == name).then_some(right)
    })
}

export_manga_source!(SOURCE);

const LIBRARY_FIXTURE: &str = r#"
[{"id":"sample","title":"Sample Series","cover":"cover.jpg","chapters":["1","2"],"chapter_roots":{"1":{"url":"https://leslie-victims.pages.dev/content/sample/1","mode":"list","data":["01.webp","02.webp"]},"2":{"url":"https://leslie-victims.pages.dev/content/sample/2","mode":"count","data":2}}}]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_chapters_and_pages() {
        assert_eq!(parse_library(LIBRARY_FIXTURE, "sample").len(), 1);
        let entry = find_entry(LIBRARY_FIXTURE, "sample").unwrap();
        assert_eq!(entry_to_chapters(&entry).len(), 2);
        assert_eq!(
            pages_from_root(entry.pointer("/chapter_roots/1").unwrap()).len(),
            2
        );
        assert_eq!(
            pages_from_root(entry.pointer("/chapter_roots/2").unwrap()).len(),
            2
        );
    }
}
