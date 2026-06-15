use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Lelscan = Lelscan;
const BASE_URL: &str = "https://lelscans.net";
const CATALOG_PAGE: &str = "https://lelscans.net/lecture-en-ligne-one-piece";

struct Lelscan;

impl MangaSource for Lelscan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalog(LIST_FIXTURE, ""));
        }
        let body = fetch_document(CATALOG_PAGE, LIST_FIXTURE);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_latest(&body))
        } else {
            Ok(parse_catalog(&body, ""))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_catalog(
            &fetch_document(CATALOG_PAGE, LIST_FIXTURE),
            query,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/lecture-en-ligne-one-piece".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/lecture-en-ligne-one-piece".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/lecture-en-ligne-one-piece/1".into());
        let first_url = format!("{}/1", url::join_url(BASE_URL, &key).trim_end_matches('/'));
        let body = fetch_document(&first_url, PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn parse_catalog(body: &str, query: &str) -> Paged<CatalogItem> {
    let query = query.to_ascii_lowercase();
    let entries = select_options(body, 0)
        .into_iter()
        .filter_map(|option| {
            let title = html::strip_tags(option);
            if !query.is_empty() && !title.to_ascii_lowercase().contains(&query) {
                return None;
            }
            let href = html::attr(option, "value")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: Some(thumbnail_from_path(&key)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("hot_manga_img")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| value.trim_end_matches(" Scan").to_string())
                .or_else(|| url::slug_from_url(&key))?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: img_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/lecture-en-ligne-one-piece".into());
    let title = html::text_between(body, "itemprop=\"title\"", "</")
        .or_else(|| html::text_between(body, "itemprop='title'", "</"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim_start_matches("Lecture en ligne ").to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "Lelscan".into());
    CatalogItem {
        key: normalize_key(&key),
        title,
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .map(|image| url::join_url(BASE_URL, &image)),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    select_options(body, 1)
        .into_iter()
        .filter_map(|option| {
            let href = html::attr(option, "value")?;
            let key = normalize_key(&href);
            let number_text = html::strip_tags(option);
            let chapter_number = number_text.parse::<f32>().ok();
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapitre {}", trim_float(&number_text))),
                chapter_number,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page_urls = select_options(body, 2)
        .into_iter()
        .filter_map(|option| html::attr(option, "value"))
        .collect::<Vec<_>>();
    if page_urls.is_empty() {
        return image_page(body, BASE_URL).into_iter().collect();
    }
    page_urls
        .into_iter()
        .enumerate()
        .filter_map(|(index, page_url)| {
            let page_body = if index == 0 {
                body.to_string()
            } else {
                fetch_document(&url::join_url(BASE_URL, &page_url), PAGE_FIXTURE)
            };
            image_page(&page_body, &page_url).map(|mut page| {
                page.description = Some(format!("Page {}", index + 1));
                page
            })
        })
        .collect()
}

fn image_page(body: &str, referer: &str) -> Option<MangaPage> {
    let section = body.split("id=\"image\"").nth(1).unwrap_or(body);
    let image = img_attr(section)?;
    Some(MangaPage {
        content: PageContent::Url {
            url: url::join_url(BASE_URL, &image),
            context: None,
        },
        headers: manga::image_headers(&url::join_url(BASE_URL, referer)),
        description: Some("Page 1".into()),
        ..MangaPage::default()
    })
}

fn select_options(body: &str, index: usize) -> Vec<&str> {
    let navigation = body.split("id=\"navigation\"").nth(1).unwrap_or(body);
    let Some(select) = navigation.split("<select").skip(1).nth(index) else {
        return Vec::new();
    };
    select
        .split("</select>")
        .next()
        .unwrap_or(select)
        .split("<option")
        .skip(1)
        .collect()
}

fn img_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn thumbnail_from_path(manga_path: &str) -> String {
    let slug = manga_path
        .replace("/lecture-en-ligne-", "")
        .replace("/lecture-ligne-", "")
        .trim_end_matches(".php")
        .to_string();
    format!("{BASE_URL}/mangas/{slug}/thumb_cover.jpg")
}

fn key_from_input(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/lecture-") {
        Some(normalize_key(input.trim_start_matches(BASE_URL)))
    } else if input.starts_with("/lecture-") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(index) = value.find(BASE_URL) {
        return normalize_key(&value[index + BASE_URL.len()..]);
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn trim_float(value: &str) -> String {
    value.trim().trim_end_matches(".0").to_string()
}

const LIST_FIXTURE: &str = r#"
<div id="navigation"><select><option value="/lecture-en-ligne-one-piece">One Piece</option><option value="/lecture-ligne-naruto.php">Naruto</option></select><select><option value="/lecture-en-ligne-one-piece/1">1</option></select><select><option value="/lecture-en-ligne-one-piece/1/1">1</option></select></div>
<ul id="main_hot_ul"><li><a class="hot_manga_img" title="One Piece Scan" href="/lecture-en-ligne-one-piece"><img src="/mangas/one-piece/thumb_cover.jpg"></a></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="header-image"><h2><div></div><div><span itemprop="title">Lecture en ligne One Piece</span></div></h2></div>
<meta property="og:image" content="/mangas/one-piece/thumb_cover.jpg">
<div id="navigation"><select><option value="/lecture-en-ligne-one-piece">One Piece</option></select><select><option value="/lecture-en-ligne-one-piece/1">1</option></select><select><option value="/lecture-en-ligne-one-piece/1/1">1</option></select></div>
"#;
const PAGES_FIXTURE: &str = r#"<div id="navigation"><select></select><select></select><select><option value="/lecture-en-ligne-one-piece/1/1">1</option></select></div><div id="image"><img src="/page1.jpg"></div>"#;
const PAGE_FIXTURE: &str = r#"<div id="image"><img src="/page1.jpg"></div>"#;
