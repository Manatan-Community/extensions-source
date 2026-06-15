use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: VinaHentai = VinaHentai;
const DEFAULT_BASE_URL: &str = "https://vinahentai.life";

struct VinaHentai;

impl MangaSource for VinaHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let sort = if listing(&request) == "popular" {
            "viewNumber"
        } else {
            "updatedAt"
        };
        parse_list_page(&fetch_document(&base, &format!("{base}/danh-sach?page={}&sort={sort}", page(&request)))?, &base)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = query(&request);
        if let Some(key) = key_from_url(&base, &query) {
            return Ok(Paged {
                entries: vec![details_by_key(&base, &key)?],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!(
                "{base}/search?page={}&q={}",
                page(&request),
                url::query_escape(&query)
            )
        } else if let Some(genre) = filter(&request, "genre") {
            format!(
                "{base}/genres/{}?page={}&sort={}",
                genre.trim_matches('/'),
                page(&request),
                filter(&request, "sort").unwrap_or("updatedAt")
            )
        } else {
            format!(
                "{base}/danh-sach?page={}&sort={}",
                page(&request),
                filter(&request, "sort").unwrap_or("updatedAt")
            )
        };
        let body = fetch_document(&base, &target)?;
        if target.contains("/search?") {
            parse_search_page(&body, &base)
        } else {
            parse_list_page(&body, &base)
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-hentai/sample".to_string());
        details_by_key(&base, &key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-hentai/sample".to_string());
        Ok(parse_chapters(&fetch_document(&base, &absolute(&base, &key))?, &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-hentai/sample/chapter-1".to_string());
        let target = absolute(&base, &key);
        Ok(parse_pages(&fetch_document(&base, &target)?, &base, &target))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let prefs = request.get("preferences").cloned().unwrap_or(Value::Null);
        let popular = self.list(json!({"page": 1, "listingId": "popular", "preferences": prefs}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "preferences": prefs}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(&base, input) {
            let is_chapter = key.trim_matches('/').matches('/').count() > 1;
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&base, &key)).transpose()?,
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base.trim_end_matches('/')))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str) -> ExtensionResult<String> {
    client(base).get(target).browser_document().send_text()
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn absolute(base: &str, value: &str) -> String {
    url::join_url(base, value)
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input
        .starts_with(base)
        .then(|| normalize_key(base, input))
        .filter(|key| key.contains("/truyen-hentai/"))
}

fn normalize_key(base: &str, input: &str) -> String {
    let value = input
        .trim()
        .trim_start_matches(base.trim_end_matches('/'))
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

fn resolve_manga_url(href: &str) -> String {
    if href.starts_with("/login") {
        if let Some(redirect) = href.split("redirect=").nth(1) {
            return percent_decode(redirect);
        }
    }
    href.to_string()
}

fn parse_list_page(body: &str, base: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("group") && (chunk.contains("/truyen-hentai/") || chunk.contains("/login?redirect=")))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(base, &resolve_manga_url(&href));
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "<h2", "</h2>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(catalog_item(base, key, title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique);
    Ok(Paged {
        entries,
        has_next_page: body.contains("title=\"Tới trang cuối\"") && !body.contains("disabled"),
    })
}

fn parse_search_page(body: &str, base: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen-hentai/") || chunk.contains("/login?redirect="))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(base, &resolve_manga_url(&href));
            let title = html::text_between(chunk, "<h2", "</h2>")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(catalog_item(base, key, title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique);
    Ok(Paged {
        entries,
        has_next_page: body.contains("href") && body.contains("page="),
    })
}

fn catalog_item(base: &str, key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute(base, &image)),
        url: Some(absolute(base, &key)),
        language: Some("vi".to_string()),
        content_rating: Some("adult".to_string()),
        ..CatalogItem::default()
    }
}

fn details_by_key(base: &str, key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details(&fetch_document(base, &absolute(base, key))?, base, key))
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    let authors = links_by_prefix(body, "/authors/");
    let tags = links_by_prefix(body, "/genres/");
    CatalogItem {
        key: normalize_key(base, key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "Bìa", "src")
            .or_else(|| html::attr_after(body, "story-images", "src"))
            .map(|image| absolute(base, &image)),
        description: html::text_between(body, "manga-description-section", "</section>")
            .or_else(|| html::text_between(body, "manga-description-section", "</div>"))
            .map(|value| html::strip_tags(&value)),
        authors: authors.clone(),
        artists: authors,
        tags,
        status: status_from(body),
        url: Some(absolute(base, key)),
        language: Some("vi".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn links_by_prefix(body: &str, prefix: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(prefix))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty() && !value.starts_with('+'))
        .fold(Vec::new(), |mut out, value| {
            if !out.contains(&value) {
                out.push(value);
            }
            out
        })
}

fn status_from(body: &str) -> ItemStatus {
    if body.contains("Đang tiến hành") {
        ItemStatus::Ongoing
    } else if body.contains("Đã hoàn thành") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("block") && chunk.contains("/truyen-hentai/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(base, &href);
            if key.trim_matches('/').matches('/').count() < 2 {
                return None;
            }
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<span", "</span>")
                    .or_else(|| Some(html::strip_tags(chunk)))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute(base, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, base: &str, referer: &str) -> Vec<MangaPage> {
    let domain = base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    body.split(['"', '\''])
        .filter(|part| part.contains("/manga-images/") && part.contains(&format!("cdn.{domain}")))
        .map(ToString::to_string)
        .fold(Vec::new(), |mut out, image| {
            if !out.contains(&image) {
                out.push(image);
            }
            out
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .filter(|value| !value.starts_with("data:"))
}

fn percent_decode(input: &str) -> String {
    let mut out = String::new();
    let mut bytes = input.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = bytes.next();
            let lo = bytes.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let Ok(value) = u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16) {
                    out.push(value as char);
                    continue;
                }
            }
            out.push('%');
        } else if byte == b'+' {
            out.push(' ');
        } else {
            out.push(byte as char);
        }
    }
    out
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_login_redirect() {
        assert_eq!(
            resolve_manga_url("/login?redirect=%2Ftruyen-hentai%2Fsample"),
            "/truyen-hentai/sample"
        );
    }

    #[test]
    fn parses_cdn_pages() {
        let pages = parse_pages(
            r#""https://cdn.vinahentai.life/manga-images/a/1.jpg""#,
            DEFAULT_BASE_URL,
            "https://vinahentai.life/truyen-hentai/a/1",
        );
        assert_eq!(pages.len(), 1);
    }
}

export_manga_source!(SOURCE);
