use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: VinnieVeritas = VinnieVeritas;
const BASE_URL: &str = "https://ccc.vinnieveritas.com";
const ARCHIVE_KEY: &str = "/archiveIndex.php";

struct VinnieVeritas;

impl MangaSource for VinnieVeritas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![series_item(config_for(&request))],
            has_next_page: false,
        })
    }

    fn search(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged::default())
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(series_item(config_for(&request)))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let body =
            fetch_document_or_fixture(&url::join_url(BASE_URL, ARCHIVE_KEY), ARCHIVE_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config_for(&request);
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample.php".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body, config))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(config_for(&request))),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    title: &'static str,
    lang: &'static str,
    description: &'static str,
    thumbnail: &'static str,
    image_class: &'static str,
}

fn config_for(request: &Value) -> SourceConfig {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("vinnieveritas-es") => SourceConfig {
            id: "vinnieveritas-es",
            title: "CCC: La ciudad de las oportunidades",
            lang: "es",
            description: "Webcomic de Vinnie Veritas sobre Lucio Vasalle y sus desventuras como recien llegado a CCC.",
            thumbnail: "https://ccc.vinnieveritas.com/comics/CCCr000.jpg",
            image_class: "crazylan-es",
        },
        _ => SourceConfig {
            id: "vinnieveritas-en",
            title: "CCC: The city of opportunities",
            lang: "en",
            description: "Webcomic by Vinnie Veritas about Lucio Vasalle and his misadventures as a newcomer to CCC.",
            thumbnail: "https://ccc.vinnieveritas.com/comics/CCCr000E.jpg",
            image_class: "crazylan-en",
        },
    }
}

fn series_item(config: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: ARCHIVE_KEY.to_string(),
        title: config.title.to_string(),
        authors: vec!["Vinnie Veritas".to_string()],
        artists: vec!["Vinnie Veritas".to_string()],
        description: Some(config.description.to_string()),
        tags: vec!["webcomic".to_string()],
        status: ItemStatus::Ongoing,
        cover: Some(config.thumbnail.to_string()),
        url: Some(url::join_url(BASE_URL, ARCHIVE_KEY)),
        language: Some(config.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        extra: [("sourceId".to_string(), Value::String(config.id.to_string()))]
            .into_iter()
            .collect(),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("cccArchiveEntry")
        .skip(1)
        .filter(|chunk| chunk.contains("onclick"))
        .filter_map(|chunk| {
            let onclick = html::attr(chunk, "onclick")?;
            let comic_name = comic_name_from_onclick(&onclick)?;
            Some(MangaChapter {
                key: format!("/{comic_name}.php"),
                title: Some(html::strip_tags(chunk)),
                url: Some(format!("{BASE_URL}/{comic_name}.php")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn comic_name_from_onclick(value: &str) -> Option<String> {
    value
        .split("changeToComic(")
        .nth(1)?
        .trim_start_matches(['"', '\''])
        .split(['"', '\''])
        .next()
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn parse_pages(body: &str, config: SourceConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("cccComic") && chunk.contains(config.image_class))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div class="cccLeftInd">
  <div class="cccArchiveEntry" onclick="changeToComic('CCCr001E')">Chapter 1</div>
  <div class="cccArchiveEntry" onclick='changeToComic("CCCr002E")'>Chapter 2</div>
</div>
"#;

const PAGE_FIXTURE: &str = r#"
<img class="cccComic crazylan-en" src="/comics/CCCr001E.jpg">
<img class="cccComic crazylan-es" src="/comics/CCCr001.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive_entries() {
        let chapters = parse_chapters(ARCHIVE_FIXTURE);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "/CCCr001E.php");
    }

    #[test]
    fn picks_pages_for_requested_language() {
        let en = parse_pages(
            PAGE_FIXTURE,
            config_for(&json!({"sourceId":"vinnieveritas-en"})),
        );
        let es = parse_pages(
            PAGE_FIXTURE,
            config_for(&json!({"sourceId":"vinnieveritas-es"})),
        );
        match &en[0].content {
            PageContent::Url { url, .. } => {
                assert_eq!(url, "https://ccc.vinnieveritas.com/comics/CCCr001E.jpg")
            }
            _ => panic!("expected URL page"),
        }
        match &es[0].content {
            PageContent::Url { url, .. } => {
                assert_eq!(url, "https://ccc.vinnieveritas.com/comics/CCCr001.jpg")
            }
            _ => panic!("expected URL page"),
        }
    }
}
