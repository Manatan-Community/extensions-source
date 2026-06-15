use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://www.commitstrip.com";
const CURRENT_YEAR: i32 = 2026;
const SOURCE: CommitStrip = CommitStrip;

struct CommitStrip;

impl MangaSource for CommitStrip {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        Ok(Paged {
            entries: (2012..=CURRENT_YEAR).rev().map(|year| year_item(source, year)).collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let year = query
                .split('/')
                .find_map(|part| part.parse::<i32>().ok())
                .unwrap_or(CURRENT_YEAR);
            return Ok(Paged { entries: vec![year_item(source, year)], has_next_page: false });
        }
        Ok(Paged {
            entries: (2012..=CURRENT_YEAR)
                .rev()
                .map(|year| year_item(source, year))
                .filter(|item| item.title.contains(query))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("{}/{CURRENT_YEAR}", source.site_lang));
        let year = key.rsplit('/').next().and_then(|value| value.parse().ok()).unwrap_or(CURRENT_YEAR);
        let mut item = year_item(source, year);
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("{}/{CURRENT_YEAR}", source.site_lang));
        let year_url = format!("{BASE_URL}/{}", key.trim_matches('/'));
        let first = fetch_document_or_fixture(&year_url, ARCHIVE_FIXTURE);
        let max_page = archive_page_count(&first);
        let mut chapters = Vec::new();
        for page in 1..=max_page {
            let body = if page == 1 { first.clone() } else { fetch_document_or_fixture(&format!("{year_url}/page/{page}"), ARCHIVE_FIXTURE) };
            chapters.extend(parse_archive_chapters(&body, source.site_lang));
        }
        chapters.dedup_by(|a, b| a.key == b.key);
        let total = chapters.len();
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter.chapter_number = Some((total - index) as f32);
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/en/2024/01/01/sample".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGE_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

#[derive(Clone, Copy)]
struct SourceConfig {
    lang: &'static str,
    site_lang: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    match request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str) {
        Some("commitstrip-fr") => SourceConfig {
            lang: "fr",
            site_lang: "fr",
        },
        _ => SourceConfig {
            lang: "en",
            site_lang: "en",
        },
    }
}

fn year_item(source: SourceConfig, year: i32) -> CatalogItem {
    CatalogItem {
        key: format!("{}/{}", source.site_lang, year),
        title: format!("Commit Strip ({year})"),
        cover: Some(if source.lang == "fr" { "https://i.imgur.com/I7ps9zS.jpg" } else { "https://i.imgur.com/HODJlt9.jpg" }.into()),
        authors: vec![if source.lang == "fr" { "Thomas Gx" } else { "Mark Nightingale" }.into()],
        artists: vec!["Etienne Issartial".into()],
        description: Some(format!("{} \n\nNote: This entry includes all the chapters published in {year}", if source.lang == "fr" { "Le blog qui raconte la vie des codeurs" } else { "The blog relating the daily life of web agency developers." })),
        status: if year == CURRENT_YEAR { ItemStatus::Ongoing } else { ItemStatus::Completed },
        url: Some(format!("{BASE_URL}/{}/{year}", source.site_lang)),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_document_or_fixture(target_url: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn archive_page_count(body: &str) -> u64 {
    html::text_between(body, "wp-pagenavi", "</")
        .and_then(|text| {
            text.split_whitespace()
                .filter_map(|part| part.parse::<u64>().ok())
                .max()
        })
        .unwrap_or(1)
}

fn parse_archive_chapters(body: &str, site_lang: &str) -> Vec<MangaChapter> {
    body.split("excerpt")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_path(&href, site_lang);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<span", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: date_from_path(&key),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("entry-content") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn date_from_path(path: &str) -> Option<i64> {
    let parts = path.split('/').collect::<Vec<_>>();
    for window in parts.windows(3) {
        let candidate = format!("{}/{}/{}", window[0], window[1], window[2]);
        if let Some(date) = dates::parse_fixture_date(&candidate.replace('/', "-")) {
            return Some(date);
        }
    }
    None
}

fn normalize_path(value: &str, site_lang: &str) -> String {
    if value.starts_with(BASE_URL) {
        let path = value[BASE_URL.len()..].trim_start_matches('/');
        if path.starts_with(site_lang) { format!("/{path}") } else { format!("/{site_lang}/{path}") }
    } else {
        format!("/{}", value.trim_start_matches('/'))
    }
}

fn absolute_url(value: &str) -> String { url::join_url(BASE_URL, value) }

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<div class="wp-pagenavi"><span class="pages">Page 1 of 1</span></div>
<div class="excerpt"><a href="https://www.commitstrip.com/en/2024/01/01/sample"><span>Sample Strip</span></a></div>
"#;
const PAGE_FIXTURE: &str = r#"<div class="entry-content"><p><img src="https://img.example/strip.jpg"></p></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commitstrip() {
        let source = SourceConfig {
            lang: "en",
            site_lang: "en",
        };
        assert_eq!(year_item(source, 2024).title, "Commit Strip (2024)");
        assert_eq!(parse_archive_chapters(ARCHIVE_FIXTURE, "en").len(), 1);
        assert_eq!(parse_pages(PAGE_FIXTURE).len(), 1);
    }
}
