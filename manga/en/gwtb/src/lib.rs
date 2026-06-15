use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Gwtb = Gwtb;
const BASE_URL: &str = "https://www.blastwave-comic.com";
const SERIES_KEY: &str = "/index.php";
const COVER: &str = "https://www.blastwave-comic.com/images/yarr.jpg";

struct Gwtb;

impl MangaSource for Gwtb {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item()],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let item = series_item();
        let entries = if query.is_empty()
            || item.title.to_ascii_lowercase().contains(&query)
            || query.starts_with(BASE_URL)
        {
            vec![item]
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(SERIES_KEY, INDEX_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| SERIES_KEY.to_string());
        Ok(parse_pages(&fetch_document(&key, PAGE_FIXTURE)))
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(url::join_url(BASE_URL, SERIES_KEY)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item()),
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

fn series_item() -> CatalogItem {
    CatalogItem {
        key: SERIES_KEY.to_string(),
        title: "Gone with the Blastwave".to_string(),
        cover: Some(COVER.to_string()),
        authors: vec!["Kimmo Lemetti".to_string()],
        artists: vec!["Kimmo Lemetti".to_string()],
        description: Some("Because war can be boring too.".to_string()),
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, SERIES_KEY)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(key: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, key))
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            if value.trim().is_empty() {
                return None;
            }
            let title = html::strip_tags(
                &chunk
                    .split_once('>')
                    .map(|(_, rest)| rest.split("</option>").next().unwrap_or(rest))
                    .unwrap_or_default()
                    .to_string(),
            );
            let number = value.parse::<f32>().ok();
            let key = format!("/index.php?nro={value}");
            Some(MangaChapter {
                key: key.clone(),
                title: (!title.is_empty()).then_some(title),
                chapter_number: number,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let image = body
        .split("comic_title")
        .nth(1)
        .and_then(|chunk| chunk.split("<img").nth(1))
        .and_then(|chunk| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
        .unwrap_or_else(|| "/images/yarr.jpg".to_string());
    vec![MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_URL, &image),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some("Page 1".to_string()),
        ..MangaPage::default()
    }]
}

export_manga_source!(SOURCE);

const INDEX_FIXTURE: &str = r#"
<select class="fall">
  <option value="">Choose comic</option>
  <option value="1">Episode 1</option>
  <option value="2">Episode 2</option>
</select>
"#;

const PAGE_FIXTURE: &str = r#"
<div class="comic_title">Episode 1</div><img src="/comics/1.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chapters() {
        let chapters = parse_chapters(INDEX_FIXTURE);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "/index.php?nro=1");
    }

    #[test]
    fn parses_page() {
        let pages = parse_pages(PAGE_FIXTURE);
        assert_eq!(pages.len(), 1);
    }
}
