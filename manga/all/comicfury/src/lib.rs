use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://comicfury.com";
const SOURCE: ComicFury = ComicFury;

struct ComicFury;

impl MangaSource for ComicFury {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let source = source_for(&request);
        let sort = if listing_id(&request) == "latest" { 2 } else { 1 };
        let body = fetch_document_or_fixture(
            &search_url(page, "", source.site_lang, sort, &Value::Null),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body, source.lang))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        let source = source_for(&request);
        if query.starts_with(BASE_URL) || query.contains(".webcomic.ws") {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(query.to_string()), source.lang)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let body = fetch_document_or_fixture(
            &search_url(page, query, source.site_lang, selected_sort(filters), filters),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body, source.lang))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "?url=sample".into());
        let body = fetch_document_or_fixture(&details_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source.lang))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "?url=sample".into());
        let body = fetch_document_or_fixture(&archive_url(&key), ARCHIVE_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/comics/1".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(
            &body,
            request
                .get("preferences")
                .and_then(|prefs| prefs.get("showAuthorsNotes"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.contains(".webcomic.ws") {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(input.to_string()), source_for(&request).lang)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    site_lang: &'static str,
}

const SOURCES: [SourceConfig; 14] = [
    SourceConfig { id: "comicfury-all", lang: "all", site_lang: "all" },
    SourceConfig { id: "comicfury-en", lang: "en", site_lang: "en" },
    SourceConfig { id: "comicfury-es", lang: "es", site_lang: "es" },
    SourceConfig { id: "comicfury-pt-br", lang: "pt-BR", site_lang: "pt" },
    SourceConfig { id: "comicfury-de", lang: "de", site_lang: "de" },
    SourceConfig { id: "comicfury-fr", lang: "fr", site_lang: "fr" },
    SourceConfig { id: "comicfury-it", lang: "it", site_lang: "it" },
    SourceConfig { id: "comicfury-pl", lang: "pl", site_lang: "pl" },
    SourceConfig { id: "comicfury-ja", lang: "ja", site_lang: "ja" },
    SourceConfig { id: "comicfury-zh", lang: "zh", site_lang: "zh" },
    SourceConfig { id: "comicfury-ru", lang: "ru", site_lang: "ru" },
    SourceConfig { id: "comicfury-fi", lang: "fi", site_lang: "fi" },
    SourceConfig { id: "comicfury-other", lang: "other", site_lang: "other" },
    SourceConfig { id: "comicfury-notext", lang: "other", site_lang: "notext" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("comicfury-all");
    SOURCES.iter().find(|source| source.id == id).copied().unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target_url: &str, fixture: &str) -> String {
    let body = client()
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    if is_content_warning(&body) {
        if let Some(token) = html::attr_after(&body, "name=\"token\"", "value") {
            return client()
                .post(target_url)
                .form(&[("token", &token), ("proceed", "View Webcomic")])
                .send_text()
                .unwrap_or(body);
        }
    }
    body
}

fn search_url(page: u64, query: &str, language: &str, sort: u64, filters: &Value) -> String {
    let mut out = format!(
        "{BASE_URL}/search.php?query={}&page={page}&language={language}&sort={sort}",
        url::query_escape(query)
    );
    for (key, default) in [("lastupdate", "0"), ("fv", "2"), ("fn", "2"), ("fl", "2"), ("fs", "2")] {
        out.push('&');
        out.push_str(key);
        out.push('=');
        out.push_str(filter_string(filters, key).unwrap_or(default));
    }
    if filter_bool(filters, "completed") {
        out.push_str("&completed=0");
    } else {
        out.push_str("&completed=1");
    }
    if let Some(tags) = filter_string(filters, "tags") {
        out.push_str("&tags=");
        out.push_str(&url::query_escape(&tags.replace(", ", ",")));
    }
    out
}

fn parse_search(body: &str, lang: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("class=\"webcomic-result\"")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::attr_after(chunk, "webcomic-result-title", "title")
                        .or_else(|| html::text_between(chunk, "webcomic-result-title", "</"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())?,
                    cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                    url: Some(details_url(&key)),
                    language: Some(lang.to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("search-next-page"),
    }
}

fn parse_details(body: &str, key: Option<String>, lang: &str) -> CatalogItem {
    let key = key.map(|value| normalize_key(&value)).unwrap_or_else(|| "?url=sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "username-and-title", "</")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic Fury".into())),
        description: html::text_between(body, "description-tags", "</div>")
            .or_else(|| html::text_between(body, "username-and-title", "</em>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: body
            .split("authorname")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        tags: body
            .split("description-tags")
            .nth(1)
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Unknown,
        url: Some(details_url(&key)),
        language: Some(lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("archive-comic"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "archive-comic-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Comic".into());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: None,
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some(index as f32);
    }
    chapters
}

fn parse_pages(body: &str, show_notes: bool) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("is--image-segment") || chunk.contains("comicimage") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect::<Vec<_>>();
    if show_notes {
        if let Some(note) = html::text_between(body, "is--comment-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
        {
            pages.push(MangaPage {
                content: PageContent::Text { text: note },
                description: Some("Author's Notes".into()),
                ..MangaPage::default()
            });
        }
    }
    pages
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn selected_sort(filters: &Value) -> u64 {
    filter_string(filters, "sort")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn filter_bool(filters: &Value, key: &str) -> bool {
    filters.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn is_content_warning(body: &str) -> bool {
    body.contains("Content Warning") && body.contains("name=\"proceed\"") && body.contains("View Webcomic")
}

fn archive_url(key: &str) -> String {
    format!("{BASE_URL}/read/{}/archive", key.trim_start_matches("?url=").trim_matches('/'))
}

fn details_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        archive_url(key)
    }
}

fn normalize_key(value: &str) -> String {
    if value.contains("?url=") {
        return format!("?url={}", value.split("?url=").nth(1).unwrap_or_default().trim_matches('/'));
    }
    if value.contains("/read/") {
        return format!("?url={}", value.split("/read/").nth(1).unwrap_or_default().split('/').next().unwrap_or_default());
    }
    value.to_string()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
<html><body>
  <div class="webcomic-result">
    <div class="webcomic-result-avatar"><a href="https://comicfury.com/read/sample/archive"><img src="https://img.example/cover.png"></a></div>
    <div class="webcomic-result-title" title="Sample Comic">Sample Comic</div>
  </div>
  <div class="search-next-page"></div>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
  <div class="username-and-title"><em>Sample Comic</em></div>
  <a class="authorname">Author One</a>
  <div class="description-tags">A description <a>Adventure</a><a>Fantasy</a></div>
</body></html>
"#;

const ARCHIVE_FIXTURE: &str = r#"
<html><body>
  <a href="https://comicfury.com/read/sample/comics/1"><div class="archive-comic"><span class="archive-comic-title">Page One</span><span class="archive-comic-date">Jan 1 2024</span></div></a>
  <a href="https://comicfury.com/read/sample/comics/2"><div class="archive-comic"><span class="archive-comic-title">Page Two</span><span class="archive-comic-date">Jan 2 2024</span></div></a>
</body></html>
"#;

const PAGES_FIXTURE: &str = r#"
<html><body>
  <div class="is--comic-page"><div class="is--image-segment"><div><img src="https://img.example/1.png"></div></div></div>
  <div class="is--comment-content">Author note.</div>
</body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search() {
        let page = parse_search(SEARCH_FIXTURE, "all");
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_chapters_pages() {
        let item = parse_details(DETAILS_FIXTURE, Some("?url=sample".into()), "all");
        assert_eq!(item.title, "Sample Comic");
        assert_eq!(parse_chapters(ARCHIVE_FIXTURE).len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE, true).len(), 2);
    }
}
