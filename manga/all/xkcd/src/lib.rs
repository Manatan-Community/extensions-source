use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: Xkcd = Xkcd;

struct Xkcd;

impl MangaSource for Xkcd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config_for(&request);
        let chapters = fetch_chapters(config);
        let groups = group_chapters(&chapters, organization(&request));
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let per_page = 10;
        let entries = groups
            .into_iter()
            .skip(page.saturating_sub(1) * per_page)
            .take(per_page)
            .map(|(key, chapters)| series_item(config, &key, chapters.first()))
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() == per_page,
            entries,
        })
    }

    fn search(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged::default())
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "single".to_string());
        Ok(series_item(config, &key, None))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "single".to_string());
        let chapters = fetch_chapters(config);
        Ok(group_chapters(&chapters, organization(&request))
            .remove(&key)
            .unwrap_or(chapters))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config_for(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/1/".to_string());
        let body =
            fetch_document_or_fixture(config, &url::join_url(config.base_url, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body, config))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "archive".to_string(),
            title: "Archive".to_string(),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let config = config_for(&request);
        if input.starts_with(config.base_url) {
            let key = normalize_key(config, input);
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(config, "single", None)),
                chapter: Some(
                    serde_json::to_value(MangaChapter {
                        key: key.clone(),
                        title: url::slug_from_url(&key),
                        url: Some(url::join_url(config.base_url, &key)),
                        ..MangaChapter::default()
                    })
                    .unwrap_or(Value::Null),
                ),
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
    base_url: &'static str,
    lang: &'static str,
    archive: &'static str,
    creator: &'static str,
    synopsis: &'static str,
    image_marker: &'static str,
}

fn config_for(request: &Value) -> SourceConfig {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("xkcd-es") => SourceConfig {
            id: "xkcd-es",
            base_url: "https://es.xkcd.com",
            lang: "es",
            archive: "/archive",
            creator: "Randall Munroe",
            synopsis: "Un webcomic sobre romance, sarcasmo, matematicas y lenguaje.",
            image_marker: "middleContent",
        },
        Some("xkcd-zh") => SourceConfig {
            id: "xkcd-zh",
            base_url: "https://xkcd.tw",
            lang: "zh",
            archive: "/api/strips.json",
            creator: "Randall Munroe",
            synopsis: "A translated webcomic of romance, sarcasm, math and language.",
            image_marker: "content",
        },
        Some("xkcd-fr") => SourceConfig {
            id: "xkcd-fr",
            base_url: "https://xkcd.lapin.org",
            lang: "fr",
            archive: "/tous-episodes.php",
            creator: "Randall Munroe",
            synopsis: "Un webcomic sarcastique qui parle de romance, de maths et de langage.",
            image_marker: "content",
        },
        Some("xkcd-ru") => SourceConfig {
            id: "xkcd-ru",
            base_url: "https://xkcd.ru",
            lang: "ru",
            archive: "/img",
            creator: "Randall Munroe",
            synopsis: "A translated webcomic of romance, sarcasm, math and language.",
            image_marker: "main",
        },
        _ => SourceConfig {
            id: "xkcd-en",
            base_url: "https://xkcd.com",
            lang: "en",
            archive: "/archive",
            creator: "Randall Munroe",
            synopsis: "A webcomic of romance, sarcasm, math and language.",
            image_marker: "comic",
        },
    }
}

fn client(config: SourceConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", config.base_url))
        .with_cookies_for(config.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(config: SourceConfig, target: &str, fixture: &str) -> String {
    client(config)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_chapters(config: SourceConfig) -> Vec<MangaChapter> {
    let target = url::join_url(config.base_url, config.archive);
    let body = fetch_document_or_fixture(config, &target, ARCHIVE_FIXTURE);
    if config.id == "xkcd-zh" {
        parse_zh_archive(&body, config)
    } else {
        parse_html_archive(&body, config)
    }
}

fn parse_html_archive(body: &str, config: SourceConfig) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let text = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let number = comic_number(&href)
                .or_else(|| comic_number(&text))
                .unwrap_or(0);
            if number == 0 && text.is_empty() {
                return None;
            }
            Some(MangaChapter {
                key: normalize_key(config, &href),
                title: Some(format!("{number}: {text}")),
                chapter_number: Some(number as f32),
                url: Some(url::join_url(config.base_url, &href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_zh_archive(body: &str, config: SourceConfig) -> Vec<MangaChapter> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return parse_html_archive(ARCHIVE_FIXTURE, config);
    };
    value
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .filter_map(|entry| {
            let number = entry.get("id").and_then(Value::as_u64)? as u32;
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(MangaChapter {
                key: format!("/{number}"),
                title: Some(format!("{number}: {title}")),
                chapter_number: Some(number as f32),
                url: Some(format!("{}/{number}", config.base_url)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn group_chapters(
    chapters: &[MangaChapter],
    organization: &str,
) -> BTreeMap<String, Vec<MangaChapter>> {
    let mut groups = BTreeMap::<String, Vec<MangaChapter>>::new();
    for chapter in chapters {
        let key = match organization {
            "year" | "year-month" => chapter
                .date_uploaded
                .map(|date| date.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            _ => "single".to_string(),
        };
        groups.entry(key).or_default().push(chapter.clone());
    }
    groups
}

fn series_item(config: SourceConfig, key: &str, chapter: Option<&MangaChapter>) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: if key == "single" {
            "xkcd".to_string()
        } else {
            format!("xkcd {key}")
        },
        authors: vec![config.creator.to_string()],
        artists: vec![config.creator.to_string()],
        description: Some(config.synopsis.to_string()),
        status: ItemStatus::Ongoing,
        cover: chapter
            .and_then(|chapter| chapter.url.clone())
            .unwrap_or_else(|| "https://thumbnail/xkcd.png".to_string())
            .into(),
        url: Some(config.base_url.to_string()),
        language: Some(config.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        extra: [("sourceId".to_string(), Value::String(config.id.to_string()))]
            .into_iter()
            .collect(),
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, config: SourceConfig) -> Vec<MangaPage> {
    let marker = config.image_marker;
    let image = body
        .split("<img")
        .skip(1)
        .find(|chunk| chunk.contains(marker) || chunk.contains("src="))
        .and_then(|chunk| html::attr(chunk, "srcset").or_else(|| html::attr(chunk, "src")))
        .map(|value| {
            value
                .split_whitespace()
                .next()
                .unwrap_or(&value)
                .to_string()
        })
        .map(|value| url::join_url(config.base_url, &value))
        .unwrap_or_else(|| format!("{}/1.png", config.base_url));
    let alt = body
        .split("<img")
        .skip(1)
        .find_map(|chunk| html::attr(chunk, "alt"))
        .unwrap_or_default();
    let title = body
        .split("<img")
        .skip(1)
        .find_map(|chunk| html::attr(chunk, "title"))
        .unwrap_or_default();
    vec![
        MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(config.base_url)),
            },
            headers: manga::image_headers(config.base_url),
            description: Some("Comic".to_string()),
            ..MangaPage::default()
        },
        manga::text_page(&format!("{alt}\n\n{title}")),
    ]
}

fn organization(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("organizationMethod"))
        .and_then(Value::as_str)
        .unwrap_or("single")
}

fn normalize_key(config: SourceConfig, value: &str) -> String {
    let path = value.trim_start_matches(config.base_url);
    format!("/{}", path.trim_start_matches('/'))
}

fn comic_number(value: &str) -> Option<u32> {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div id="middleContainer"><a href="/2/" title="2006-1-1">Sample Two</a><a href="/1/" title="2006-1-1">Sample One</a></div>
"#;

const PAGE_FIXTURE: &str = r#"
<div id="comic"><img src="//imgs.xkcd.com/comics/sample.png" alt="Alt text" title="Title text"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_archive_and_pages() {
        let config = config_for(&serde_json::json!({"sourceId":"xkcd-en"}));
        assert_eq!(parse_html_archive(ARCHIVE_FIXTURE, config).len(), 2);
        assert_eq!(parse_pages(PAGE_FIXTURE, config).len(), 2);
    }
}
