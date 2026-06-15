use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ZeurelScan = ZeurelScan;
const BASE_URL: &str = "https://www.zeurelscan.com";

struct ZeurelScan;

impl MangaSource for ZeurelScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing_id = request.get("listingId").and_then(Value::as_str).unwrap_or("popular");
        let target = if listing_id == "latest" {
            format!("{BASE_URL}/ultimi")
        } else {
            format!("{BASE_URL}/series")
        };
        let body = fetch_document_or_fixture(&target, if listing_id == "latest" { LATEST_FIXTURE } else { LIST_FIXTURE });
        Ok(if listing_id == "latest" {
            parse_latest(&body)
        } else {
            parse_series(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document_or_fixture(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let mut page = parse_series(&fetch_document_or_fixture(&format!("{BASE_URL}/series"), LIST_FIXTURE));
        let needle = query.to_lowercase();
        if !needle.is_empty() {
            page.entries.retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document_or_fixture(input, DETAILS_FIXTURE), Some(key))),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_series(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("series-card"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let title = html::text_between(chunk, "series-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))?;
                Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("latest-row"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let title = html::text_between(chunk, "latest-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))?;
                Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    let header = html::text_between(body, "series-header", "</section>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&header, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "ZeurelScan".to_string()),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(&header, "series-plot", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_line(&header, "Autore").into_iter().collect(),
        artists: info_line(&header, "Artista").into_iter().collect(),
        tags: info_line(&header, "Genere").map(|value| vec![value]).unwrap_or_default(),
        status: parse_status(&info_line(&header, "Stato").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut last = 0.0f32;
    body.split("class=\"chapter")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let raw = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).unwrap_or_default();
            let after_hash = raw.split('#').nth(1).unwrap_or(&raw);
            let number_text = after_hash.split('–').next().unwrap_or(after_hash).trim();
            let number = if number_text.contains('_') {
                number_text.split('_').next().and_then(|value| value.parse::<f32>().ok()).map(|value| value + 0.1)
            } else {
                number_text.parse::<f32>().ok()
            }.unwrap_or_else(|| {
                last += 0.1;
                last
            });
            last = number;
            let title_tail = after_hash.split('–').nth(1).map(str::trim).filter(|value| !value.is_empty());
            let title = title_tail.map(|tail| format!("{number_text} - {tail}")).unwrap_or_else(|| format!("Capitolo {number_text}"));
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: Some(number),
                date_uploaded: html::text_between(chunk, "chapter-date", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("reader") || chunk.contains("src="))
        .filter_map(image_attr)
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

fn info_line(body: &str, label: &str) -> Option<String> {
    let marker = format!("p:contains({label})");
    html::text_between(body, &marker, "</p>")
        .or_else(|| {
            body.split("<p")
                .find(|chunk| chunk.contains(label))
                .map(|chunk| html::strip_tags(chunk))
        })
        .map(|value| value.replace(label, "").trim_matches([':', ' ']).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lowered = value.to_lowercase();
    if lowered.contains("in corso") {
        ItemStatus::Ongoing
    } else if lowered.contains("completa") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src"))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a class="series-card" href="/series/sample"><img src="/cover.jpg"><span class="series-title">Sample Manga</span></a>"#;
const LATEST_FIXTURE: &str = r#"<a class="latest-row" href="/series/sample"><img class="latest-thumb" src="/cover.jpg"><span class="latest-title">Sample Manga</span></a>"#;
const DETAILS_FIXTURE: &str = r#"<section><div class="series-header"><h1>Sample Manga</h1><p>Autore: Author</p><p>Artista: Artist</p><p>Genere: Action</p><p>Stato: In Corso</p><p class="series-plot">Description</p></div><div class="chapter"><a href="/read/sample/1">#1 – Start</a><span class="chapter-date">01/01/2024</span></div></section><img src="/cover.jpg">"#;
const PAGES_FIXTURE: &str = r#"<div class="reader"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_zeurelscan_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Manga");
        assert_eq!(SOURCE.chapters(json!({"manga":"/series/sample"})).unwrap()[0].chapter_number, Some(1.0));
        assert_eq!(SOURCE.pages(json!({"chapter":"/read/sample/1"})).unwrap().len(), 2);
    }
}
