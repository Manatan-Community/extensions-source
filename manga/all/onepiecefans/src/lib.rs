use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: OnePieceFans = OnePieceFans;
const BASE_URL: &str = "https://one-piece-fans2.com";
const DEFAULT_THUMBNAIL_URL: &str = "https://one-piece-fans2.com/images/luffy.png";

const SOURCES: [SourceConfig; 2] = [
    SourceConfig {
        id: "onepiecefans-es",
        lang: "es",
        internal_lang: "es",
        chapter_prefix: "Chapter",
    },
    SourceConfig {
        id: "onepiecefans-en",
        lang: "en",
        internal_lang: "en",
        chapter_prefix: "Chapter",
    },
];

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    internal_lang: &'static str,
    chapter_prefix: &'static str,
}

struct OnePieceFans;

impl MangaSource for OnePieceFans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let body =
            fetch_text_or_fixture(&format!("{BASE_URL}/fansubs-config.json"), CONFIG_FIXTURE);
        Ok(Paged {
            entries: parse_config(&body, source, thumbnail_url(&request)),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let mut page = self.list(request)?;
        if !query.is_empty() {
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&query));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "fansub".into());
        Ok(CatalogItem {
            key: key.clone(),
            title: format!("One Piece ({key})"),
            cover: Some(thumbnail_url(&request)),
            url: Some(format!("{BASE_URL}/manga/{}/{key}", source.internal_lang)),
            language: Some(source.lang.to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Ongoing,
            initialized: true,
            ..CatalogItem::default()
        })
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let folder = manga::request_key(&request, "manga").unwrap_or_else(|| "fansub".into());
        let target = format!(
            "{BASE_URL}/server.php?lang={}&folderName={folder}",
            source.internal_lang
        );
        let body = fetch_text_or_fixture(&target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &folder, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "fansub/1".into());
        let (folder, chapter) = key.split_once('/').unwrap_or((&key, "1"));
        let target = format!(
            "{BASE_URL}/server.php?lang={}&folderName={folder}&chapter={chapter}",
            source.internal_lang
        );
        let body = fetch_text_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body, folder, chapter, source))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("fansub");
            return Ok(Some(UrlResolveResult {
                item: Some(CatalogItem {
                    key: key.to_string(),
                    title: format!("One Piece ({key})"),
                    cover: Some(thumbnail_url(&request)),
                    url: Some(format!("{BASE_URL}/manga/{}/{key}", source.internal_lang)),
                    language: Some(source.lang.to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Ongoing,
                    initialized: true,
                    ..CatalogItem::default()
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("onepiecefans-es");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn thumbnail_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("thumbnailUrl"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .unwrap_or(DEFAULT_THUMBNAIL_URL)
        .to_string()
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_config(body: &str, source: SourceConfig, thumbnail: String) -> Vec<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    root.get(source.internal_lang)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|fansub| {
            let path = fansub.get("path")?.as_str()?;
            let title = fansub.get("title")?.as_str()?;
            Some(CatalogItem {
                key: path.to_string(),
                title: format!("One Piece ({title})"),
                cover: Some(thumbnail.clone()),
                url: Some(format!("{BASE_URL}/manga/{}/{path}", source.internal_lang)),
                language: Some(source.lang.to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Ongoing,
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_chapters(body: &str, folder: &str, source: SourceConfig) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<String>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(|number| MangaChapter {
            key: format!("{folder}/{number}"),
            title: Some(format!("{} {number}", source.chapter_prefix)),
            chapter_number: number.parse().ok(),
            url: Some(format!(
                "{BASE_URL}/manga/{}/{folder}/{number}",
                source.internal_lang
            )),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str, folder: &str, chapter: &str, source: SourceConfig) -> Vec<MangaPage> {
    serde_json::from_str::<Vec<String>>(body)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, file_name)| {
            let image = format!(
                "{BASE_URL}/mangafiles/{}/{folder}/{chapter}/{file_name}",
                source.internal_lang
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

export_manga_source!(SOURCE);

const CONFIG_FIXTURE: &str = r#"{"es":[{"path":"fansub-es","title":"Fansub ES"}],"en":[{"path":"fansub-en","title":"Fansub EN"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"["1","2"]"#;
const PAGES_FIXTURE: &str = r#"["001.jpg","002.jpg"]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_piece_fans_fixtures() {
        let source = SOURCES[0];
        assert_eq!(
            parse_config(CONFIG_FIXTURE, source, DEFAULT_THUMBNAIL_URL.into()).len(),
            1
        );
        assert_eq!(
            parse_chapters(CHAPTERS_FIXTURE, "fansub-es", source).len(),
            2
        );
        assert_eq!(
            parse_pages(PAGES_FIXTURE, "fansub-es", "1", source).len(),
            2
        );
    }
}
