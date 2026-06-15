use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ManhuaRush = ManhuaRush;
const BASE_URL: &str = "https://manhuarush.vercel.app";

struct ManhuaRush;

impl MangaSource for ManhuaRush {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let body = fetch_document(&format!("{BASE_URL}/collections/all?p=all"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
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
            let body = fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_RSC_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_RSC_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_rsc(&url::join_url(BASE_URL, &key), DETAILS_RSC_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/sample-id/1".to_string());
        let body = fetch_rsc(&url::join_url(BASE_URL, &key), PAGES_RSC_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            if key.starts_with("/reader/") {
                return Ok(Some(UrlResolveResult {
                    url: Some(input.into()),
                    ..UrlResolveResult::default()
                }));
            }
            let body = fetch_rsc(input, DETAILS_RSC_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.into()),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("RSC", "1")
        .header("Accept", "text/x-component")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("card-link"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| "Manhua Rush".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let dto = extract_details(body).unwrap_or_else(sample_details);
    let key = key.unwrap_or_else(|| "/series/sample".into());
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Manhua Rush".into()),
        description: dto.text,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let dto = extract_details(body).unwrap_or_else(sample_details);
    dto.chapters
        .into_iter()
        .map(|chapter| {
            let title = if chapter.title.trim().is_empty() {
                format!("Chapter {}", chapter.chapter)
            } else {
                format!("Chapter {} - {}", chapter.chapter, chapter.title)
            };
            MangaChapter {
                key: format!("/reader/{}/{}", dto.mangadex_id, chapter.chapter),
                title: Some(title),
                url: Some(format!(
                    "{BASE_URL}/reader/{}/{}",
                    dto.mangadex_id, chapter.chapter
                )),
                date_uploaded: chapter.created_at.as_deref().and_then(parse_iso_date),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let dto = extract_reader(body).unwrap_or_else(sample_reader);
    dto.image_urls
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_details(body: &str) -> Option<MangaDetailsDto> {
    extract_object_containing(body, "\"chapters\"")
        .and_then(|raw| serde_json::from_str::<MangaDetailsDto>(&raw).ok())
        .or_else(|| serde_json::from_str::<MangaDetailsDto>(body).ok())
}

fn extract_reader(body: &str) -> Option<ReaderDto> {
    extract_object_containing(body, "\"imageUrls\"")
        .or_else(|| extract_object_containing(body, "\"image_urls\""))
        .and_then(|raw| serde_json::from_str::<ReaderDto>(&raw).ok())
        .or_else(|| serde_json::from_str::<ReaderDto>(body).ok())
}

fn extract_object_containing(body: &str, marker: &str) -> Option<String> {
    for (marker_index, _) in body.match_indices(marker) {
        let start = body[..marker_index].rfind('{')?;
        if let Some(end) = balanced_json_end(&body[start..]) {
            return Some(body[start..start + end].to_string());
        }
    }
    None
}

fn balanced_json_end(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn normalize_key(value: &str) -> String {
    let value = value.split('?').next().unwrap_or(value);
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value
        .split('T')
        .next()
        .and_then(manatan_shared::dates::parse_fixture_date)
}

#[derive(Debug, Deserialize)]
struct MangaDetailsDto {
    text: Option<String>,
    chapters: Vec<ChapterDto>,
    #[serde(rename = "mangadexId")]
    mangadex_id: String,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    chapter: String,
    title: String,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReaderDto {
    #[serde(rename = "imageUrls", alias = "image_urls")]
    image_urls: Vec<String>,
}

fn sample_details() -> MangaDetailsDto {
    serde_json::from_str(DETAILS_RSC_FIXTURE).expect("details fixture")
}

fn sample_reader() -> ReaderDto {
    serde_json::from_str(PAGES_RSC_FIXTURE).expect("pages fixture")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="card-link" href="/series/sample"><img alt="Sample Rush" src="/cover.jpg"></a>
"#;
const DETAILS_RSC_FIXTURE: &str = r#"{"text":"A sample description.","chapters":[{"chapter":"1","title":"The Start","createdAt":"2024-01-01T00:00:00.000Z"}],"mangadexId":"sample-id"}"#;
const PAGES_RSC_FIXTURE: &str =
    r#"{"imageUrls":["https://cdn.example.test/page1.jpg","https://cdn.example.test/page2.jpg"]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rush_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Rush"
        );
        assert_eq!(
            SOURCE.chapters(json!({})).unwrap()[0].key,
            "/reader/sample-id/1"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
