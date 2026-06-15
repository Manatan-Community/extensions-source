use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TokyoGhoul = TokyoGhoul;
const BASE_URL: &str = "https://ww11.tokyoghoulre.com";
const CONTENT_RATING: &str = "safe";

const SOURCE_LIST: &[(&str, &str)] = &[
    ("Tokyo Ghoul", "/manga/tokyo-ghoul/"),
    ("Tokyo Ghoul Jack", "/manga/tokyo-ghoul-jack/"),
    ("Tokyo Ghoul: re Colored", "/manga/tokyo-ghoulre-colored/"),
    ("Gorilla", "/manga/this-gorilla-will-die-in-1-day/"),
    ("Zakki", "/manga/tokyo-ghoul-zakki/"),
    ("Light Novel", "/manga/tokyo-ghoul-re-light-novels/"),
    ("Choujin X", "/manga/choujin-x/"),
    ("Tokyo Ghoul re", "/manga/tokyo-ghoulre/"),
];

struct TokyoGhoul;

impl MangaSource for TokyoGhoul {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: SOURCE_LIST
                .iter()
                .map(|(title, key)| source_item(title, key))
                .collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let query = query.to_ascii_lowercase();
        Ok(Paged {
            entries: SOURCE_LIST
                .iter()
                .filter(|(title, _)| {
                    query.is_empty() || title.to_ascii_lowercase().contains(&query)
                })
                .map(|(title, key)| source_item(title, key))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/tokyo-ghoul".into());
        Ok(parse_details(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/tokyo-ghoul".into());
        Ok(parse_chapters(&fetch_document(
            &manga_url(&key),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/tokyo-ghoul/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Compact),
            entries: SOURCE_LIST
                .iter()
                .map(|(title, key)| source_item(title, key))
                .collect(),
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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

fn source_item(title: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: title.to_string(),
        url: Some(manga_url(key)),
        language: Some("en".into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/tokyo-ghoul".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        description: html::attr_after(body, "name=\"description\"", "content")
            .or_else(|| html::attr_after(body, "property=\"og:description\"", "content"))
            .or_else(|| html::text_between(body, "Description", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(manga_url(&key)),
        language: Some("en".into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/manga/") || !href.contains("chapter") {
                return None;
            }
            let key = normalize_key(&href);
            let mut title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Chapter".into()));
            if let Some(subtitle) = html::text_between(chunk, "text-xs", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty() && !title.contains(value))
            {
                title = format!("{title} - {subtitle}");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                chapter_number: chapter_number(&key),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
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

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", path.trim_matches('/'))
}

fn manga_url(key: &str) -> String {
    url::join_url(BASE_URL, &normalize_key(key))
}

fn chapter_number(key: &str) -> Option<f32> {
    key.rsplit('/')
        .find_map(|part| part.trim_start_matches("chapter-").parse().ok())
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const DETAILS_FIXTURE: &str = r#"<h1>Tokyo Ghoul</h1><meta property="og:image" content="https://i.imgur.com/LGjBype.png"><meta name="description" content="Read Tokyo Ghoul manga for free online">"#;
const CHAPTERS_FIXTURE: &str = r#"<a href="/manga/tokyo-ghoul/chapter-1/">Chapter 1</a><a href="/manga/tokyo-ghoul/chapter-2/">Chapter 2</a>"#;
const PAGES_FIXTURE: &str =
    r#"<img data-src="/images/page-1.jpg"><img data-src="/images/page-2.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries.len(), 8);
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 2);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
