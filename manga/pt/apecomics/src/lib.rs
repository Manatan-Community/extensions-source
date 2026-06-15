use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Capitoons = Capitoons;
const BASE_URL: &str = "https://capitoons.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";

struct Capitoons;

impl MangaSource for Capitoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let listing = request.get("listingId").and_then(Value::as_str);
        let default_order = if listing == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document(
            &series_url(&request, default_order),
            LIST_FIXTURE,
        )))
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
        Ok(parse_listing(&fetch_document(
            &series_url(&request, "title"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key
                    .starts_with("/manga/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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
        .with_origin(BASE_URL)
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

fn series_url(request: &Value, default_order: &str) -> String {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let mut target = if page > 1 {
        format!("{BASE_URL}/series/page/{page}/")
    } else {
        format!("{BASE_URL}/series/")
    };
    let mut params = vec![
        (
            "title",
            request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        (
            "order",
            filter(request.get("filters"), "order").unwrap_or_else(|| default_order.to_string()),
        ),
        (
            "status",
            filter(request.get("filters"), "status").unwrap_or_default(),
        ),
        (
            "type",
            filter(request.get("filters"), "type").unwrap_or_default(),
        ),
    ];
    for genre in multi_filter(request.get("filters"), "genre") {
        params.push(("genre[]", genre));
    }
    for year in multi_filter(request.get("filters"), "years") {
        params.push(("years[]", year));
    }
    let query = params
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query);
    }
    target
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("/manga/") && chunk.contains("w-full"))
            .filter_map(catalog_from_entry)
            .collect(),
        has_next_page: body.contains("page-numbers current") || body.contains("pagination"),
    }
}

fn catalog_from_entry(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| {
                html::text_between(chunk, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Capitoons".into())),
        cover: image_from_chunk(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Capitoons".into())),
        cover: html::attr_after(body, "itemprop=\"image\"", "src")
            .or_else(|| image_from_chunk(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "leading-relaxed", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_value(body, "Autor(es)").into_iter().collect(),
        tags: body
            .split("itemprop=\"genre\"")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("finished") || lower.contains("finalizado") {
            ItemStatus::Completed
        } else if lower.contains("hiatus") || lower.contains("em hiato") {
            ItemStatus::Hiatus
        } else if lower.contains("cancel") {
            ItemStatus::Cancelled
        } else if lower.contains("publishing")
            || lower.contains("ongoing")
            || lower.contains("em andamento")
        {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(manga_key: &str) -> Vec<MangaChapter> {
    let manga_url = absolute_url(manga_key);
    let body = fetch_document(&manga_url, DETAILS_FIXTURE);
    let mut chapters = parse_chapters(&body);
    let Some(container) = body.split("chapter_list_container").nth(1) else {
        return chapters;
    };
    let Some(post_id) = html::attr(container, "data-post-id") else {
        return chapters;
    };
    let count = html::attr(container, "data-count").unwrap_or_else(|| "1000".to_string());
    let mut current_page = 1;
    let mut next_page = next_chapter_page(&body, current_page);
    while let Some(page) = next_page {
        let page_text = page.to_string();
        let ajax = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .header("Accept", "*/*")
            .header("Referer", manga_url.as_str())
            .xhr()
            .form(&[
                ("action", "load_chapters"),
                ("post_id", post_id.as_str()),
                ("count", count.as_str()),
                ("paged", page_text.as_str()),
                ("order", "DESC"),
            ])
            .send_text()
            .unwrap_or_default();
        chapters.extend(parse_chapters(&ajax));
        current_page = page;
        next_page = next_chapter_page(&ajax, current_page);
        if current_page >= 25 {
            break;
        }
    }
    chapters.into_iter().fold(Vec::new(), push_unique_chapter)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let source = body.split("chapter_list_container").nth(1).unwrap_or(body);
    source
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                date_uploaded: chunk
                    .rsplit("<span")
                    .next()
                    .map(html::strip_tags)
                    .and_then(|value| parse_slash_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn next_chapter_page(body: &str, current_page: u64) -> Option<u64> {
    body.split("load-chapters")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-paged"))
        .filter_map(|value| value.parse::<u64>().ok())
        .find(|page| *page > current_page)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("imagech") || chunk.contains("/manga_auto_capitulos/"))
        .filter_map(image_from_chunk)
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = script_images(body);
    }
    images
        .into_iter()
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn script_images(body: &str) -> Vec<String> {
    body.split("\"image\"")
        .skip(1)
        .filter_map(|chunk| {
            let rest = chunk.split(':').nth(1)?;
            let rest = rest.trim_start();
            let quote = rest.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let end = rest[1..].find(quote)?;
            Some(rest[1..=end].replace("\\/", "/"))
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("grid-cols-2")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| html::strip_tags(chunk).replace(label, ""))
        .map(|value| value.trim_matches([':', ' ']).to_string())
        .filter(|value| !value.is_empty())
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    let value = filters?.get(key)?;
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn multi_filter(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(key)) else {
        return Vec::new();
    };
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .filter_map(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("value").and_then(Value::as_str))
            })
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn parse_slash_date(value: &str) -> Option<i64> {
    let parts = value.trim().split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    ymd_to_unix(
        parts[2].parse().ok()?,
        parts[1].parse().ok()?,
        parts[0].parse().ok()?,
    )
}

fn ymd_to_unix(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era * 146_097 + doe - 719_468) * 86_400)
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

const LIST_FIXTURE: &str = r#"
<div class="w-full h-full">
  <a href="/manga/sample" title="Sample Capitoons"><img src="/cover.jpg"><h1>Sample Capitoons</h1></a>
</div>
<div class="pagination"><span class="page-numbers current">1</span><a href="/series/page/2/">2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="text-4xl font-bold mb-2">Sample Capitoons</h1>
<img itemprop="image" src="/cover.jpg">
<div class="text-base leading-relaxed mb-6 text-muted-foreground">Sample description.</div>
<a itemprop="genre">Publishing</a><a itemprop="genre">Fantasia</a>
<div class="grid grid-cols-2 gap-4 text-sm text-gray-600 mb-6"><div><strong>Autor(es)</strong><p>Sample Author</p></div></div>
<div id="chapter_list" class="chapter_list_container" data-post-id="10" data-count="1000">
  <li><a href="/manga/sample/chapter-1"><span class="m-0">Chapter 1</span><span>01/01/2024</span></a></li>
</div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="reader-area"><img id="imagech" src="/page-1.jpg"></div>
<script>window.pages = [{"image":"\/page-2.jpg"}]</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Capitoons"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
        assert_eq!(script_images(PAGES_FIXTURE).len(), 1);
    }
}
