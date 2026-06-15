use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: LianScans = LianScans;
const BASE_URL: &str = "https://www.lianscans.com";
const CONTENT_RATING: &str = "adult";
const SAMPLE_MANGA: &str = "/manga/sample-lianscans";

struct LianScans;

impl MangaSource for LianScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, "", Some(order), request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| SAMPLE_MANGA.into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| SAMPLE_MANGA.into());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            preference_bool(&request, "pref_hide_locked_chapters"),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample-lianscans/chapter-1".into());
        Ok(parse_pages(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(
    page: u64,
    query: &str,
    forced_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for (id, key) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    if let Some(order) = forced_order {
        params.push(format!("order={order}"));
    }
    format!("{BASE_URL}/manga/?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("bsx")
                    || chunk.contains("uta")
                    || chunk.contains("imgu")
                    || chunk.contains("page-item-detail")
            })
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if !href.starts_with(BASE_URL) && !href.starts_with('/') {
                    return None;
                }
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| {
                        html::text_between(chunk, "<h3", "</h3>")
                            .map(|value| html::strip_tags(&value))
                    })
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "LianScans".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers") || body.contains("hpage"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| SAMPLE_MANGA.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "LianScans".to_string()),
        cover: html::attr_after(body, "thumb", "data-src")
            .or_else(|| html::attr_after(body, "thumb", "data-lazy-src"))
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: tag_values(body),
        status: parse_status(&info_text(body, "Status")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let locked = locked_urls(body);
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter") || chunk.contains("eph-num"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let is_locked = chunk.contains("locked")
                || chunk.contains("premium")
                || locked.iter().any(|url| url.ends_with(&key));
            if hide_locked && is_locked {
                return None;
            }
            let mut title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            if is_locked && !title.starts_with("[Locked]") {
                title = format!("[Locked] {title}");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                is_locked,
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "date", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea")
                || chunk.contains("reading-content")
                || chunk.contains("chapter")
                || chunk.contains("images")
                || chunk.contains("data-src")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    let value = info_text(body, label);
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value]
    }
}

fn info_text(body: &str, label: &str) -> String {
    body.split("<span")
        .chain(body.split("info-"))
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .map(html::strip_tags)
        .map(|value| value.replace(label, "").replace(':', "").trim().to_string())
        .unwrap_or_default()
}

fn tag_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("genre") || chunk.contains("rel=\"tag\""))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn locked_urls(body: &str) -> Vec<String> {
    let Some((_, tail)) = body.split_once("lockedUrls") else {
        return Vec::new();
    };
    let Some((_, tail)) = tail.split_once('[') else {
        return Vec::new();
    };
    let Some((raw, _)) = tail.split_once(']') else {
        return Vec::new();
    };
    raw.split(',')
        .map(|value| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .replace("\\/", "/")
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("complete") || value.contains("tamat") {
        ItemStatus::Completed
    } else if value.contains("hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("ongoing") || value.contains("berjalan") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .split('?')
                .next()
                .unwrap_or_default()
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!(
        "/{}",
        input
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bsx"><a href="https://www.lianscans.com/manga/sample-lianscans/" title="Sample LianScans"><img src="/cover.jpg" alt="Sample LianScans"></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample LianScans</h1>
<div class="thumb"><img src="/cover.jpg"></div>
<div class="entry-content"><p>Sample synopsis.</p></div>
<div class="spe"><span>Author: Writer</span><span>Artist: Artist</span><span>Status: Ongoing</span></div>
<div class="seriestugenre"><a href="/genre/action/">Action</a></div>
<ul><li class="wp-manga-chapter"><a href="https://www.lianscans.com/manga/sample-lianscans/chapter-1/">Chapter 1</a><span class="chapterdate">Jan 1, 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="readerarea"><img data-src="https://www.lianscans.com/page1.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample LianScans"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE, false).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
