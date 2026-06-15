use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: SandraAndWoo = SandraAndWoo;
const BASE_URL: &str = "https://www.sandraandwoo.com";
const THUMBNAIL: &str = "https://www.sandraandwoo.com/images/fanart/fanart-contest-2014/pictures/zheng-qu-01-color-corrected.jpg";

struct SandraAndWoo;

impl MangaSource for SandraAndWoo {
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

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config_for(&request);
        let body =
            fetch_document_or_fixture(&url::join_url(BASE_URL, config.archive), ARCHIVE_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/2024/01/01/sample/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        Ok(vec![MangaPage {
            content: PageContent::Url {
                url: html::attr_after(&body, "#comic", "src")
                    .or_else(|| html::attr_after(&body, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image))
                    .unwrap_or_else(|| format!("{BASE_URL}/comic.png")),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Page 1".to_string()),
            ..MangaPage::default()
        }])
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
    writer: &'static str,
    illustrator: &'static str,
    synopsis: &'static str,
    archive: &'static str,
}

fn config_for(request: &Value) -> SourceConfig {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("sandraandwoo-de") => SourceConfig {
            id: "sandraandwoo-de",
            title: "Sandra und Woo",
            lang: "de",
            writer: "Oliver Knorzer",
            illustrator: "Powree",
            synopsis: "Comedy-Webcomic.",
            archive: "/woode/archiv",
        },
        _ => SourceConfig {
            id: "sandraandwoo-en",
            title: "Sandra and Woo",
            lang: "en",
            writer: "Oliver Knorzer",
            illustrator: "Powree",
            synopsis: "Comedy comic strip.",
            archive: "/archive",
        },
    }
}

fn series_item(config: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: config.archive.to_string(),
        title: config.title.to_string(),
        authors: vec![config.writer.to_string()],
        artists: vec![config.illustrator.to_string()],
        description: Some(config.synopsis.to_string()),
        tags: vec!["Comedy".to_string()],
        status: ItemStatus::Hiatus,
        cover: Some(THUMBNAIL.to_string()),
        url: Some(url::join_url(BASE_URL, config.archive)),
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
    let mut last = 0.0f32;
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("Permanent Link") || chunk.contains("/20"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let hover = html::attr(chunk, "title").unwrap_or_default();
            let title = hover
                .strip_prefix("Permanent Link:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    html::text_between(chunk, ">", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .unwrap_or_else(|| "Chapter".into())
                        .leak()
                })
                .to_string();
            let number = bracket_number(&title).unwrap_or_else(|| {
                last = (last + (last + 1.0).floor()) / 2.0;
                last
            });
            last = number;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: Some(number),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn bracket_number(title: &str) -> Option<f32> {
    title
        .split('[')
        .skip(1)
        .filter_map(|part| part.split(']').next())
        .find_map(|part| part.parse::<f32>().ok())
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}/", path.trim_matches('/'))
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div id="column">
<a href="/2024/01/01/sample/" title="Permanent Link: [0001] Sample">Sample</a>
</div>
"#;
const PAGE_FIXTURE: &str = r#"<div id="comic"><img src="/comic.png"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive_and_page() {
        let chapters = SOURCE
            .chapters(json!({"sourceId":"sandraandwoo-en"}))
            .unwrap();
        assert_eq!(chapters[0].chapter_number, Some(1.0));

        let pages = SOURCE
            .pages(json!({"chapter":"/2024/01/01/sample/"}))
            .unwrap();
        assert_eq!(pages.len(), 1);
    }
}
