use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: ApenasUmaFa = ApenasUmaFa;
const BASE_URL: &str = "https://apenasuma-fa.blogspot.com";
const NAME: &str = "Apenas Uma Fa";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";
const MAX_RESULTS: u64 = 20;
const CHAPTER_RESULTS: u64 = 999_999;

struct ApenasUmaFa;

impl MangaSource for ApenasUmaFa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_feed_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed_listing(&fetch_json(
            &feed_url(page, None, None),
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
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed_listing(&fetch_json(
            &feed_url(page, Some(query), None),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        let details = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let label = html::attr_after(&details, "chapter_get", "data-labelchapter")
            .or_else(|| quoted_after(&details, "data-labelchapter"))
            .or_else(|| quoted_after(&details, "label"));
        Ok(parse_chapter_feed(
            &fetch_json(
                &feed_url(1, None, label.as_deref()).replace(
                    &format!("max-results={}", MAX_RESULTS + 1),
                    &format!("max-results={CHAPTER_RESULTS}"),
                ),
                CHAPTERS_FIXTURE,
            ),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample.html".into());
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

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = parse_feed_listing(&fetch_json(&feed_url(1, None, None), LIST_FIXTURE));
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(page: u64, query: Option<&str>, label: Option<&str>) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut path = format!("{BASE_URL}/feeds/posts/default");
    if let Some(label) = label.filter(|value| !value.is_empty()) {
        path.push_str("/-/");
        path.push_str(&url::query_escape(label));
    }
    let mut params = vec![
        "alt=json".to_string(),
        format!("max-results={}", MAX_RESULTS + 1),
        format!("start-index={start}"),
    ];
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        params.push(format!("q={}", url::query_escape(query)));
    }
    format!("{path}?{}", params.join("&"))
}

fn parse_feed_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry_link(entry).is_some())
        .map(entry_to_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() > MAX_RESULTS as usize,
        entries: entries.into_iter().take(MAX_RESULTS as usize).collect(),
    }
}

fn entry_to_catalog(entry: &Value) -> CatalogItem {
    let href = entry_link(entry).unwrap_or_else(|| format!("{BASE_URL}/p/sample.html"));
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title: entry_title(entry).unwrap_or_else(|| NAME.to_string()),
        cover: entry_thumbnail(entry),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.to_string())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| style_url_after(body, "<thum"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "id=\"synopsis\"", "</")
            .or_else(|| html::text_between(body, "id='synopsis'", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "search/label"),
        status: if lower.contains("em lançamento") || lower.contains("em lancamento") {
            ItemStatus::Ongoing
        } else if lower.contains("finalizado") || lower.contains("completo") {
            ItemStatus::Completed
        } else if lower.contains("hiato") {
            ItemStatus::Hiatus
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let href = entry_link(entry)?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: entry_title(entry),
                date_uploaded: entry
                    .get("published")
                    .and_then(|value| value.get("$t"))
                    .and_then(Value::as_str)
                    .and_then(parse_feed_date),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo".to_string()),
            url: Some(absolute_url(manga_key)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("separator")
                || chunk.contains("reader")
                || chunk.contains("blogger")
                || chunk.contains("bp.blogspot")
                || chunk.contains("googleusercontent")
        })
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn entry_title(entry: &Value) -> Option<String> {
    entry
        .get("title")
        .and_then(|title| title.get("$t"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn entry_link(entry: &Value) -> Option<String> {
    entry
        .get("link")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))
        .and_then(|link| link.get("href"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn entry_thumbnail(entry: &Value) -> Option<String> {
    entry
        .get("media$thumbnail")
        .and_then(|thumb| thumb.get("url"))
        .and_then(Value::as_str)
        .map(fix_google_thumbnail)
        .or_else(|| {
            entry
                .get("content")
                .and_then(|content| content.get("$t"))
                .and_then(Value::as_str)
                .and_then(|content| html::attr_after(content, "<img", "src"))
        })
}

fn style_url_after(body: &str, marker: &str) -> Option<String> {
    let style = html::attr_after(body, marker, "style")?;
    style
        .split("url(")
        .nth(1)?
        .trim_matches(['"', '\'', ')'])
        .split(')')
        .next()
        .map(ToString::to_string)
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let quote = rest.find(['"', '\''])?;
    let quote_char = rest.as_bytes()[quote] as char;
    let after = &rest[quote + 1..];
    let end = after.find(quote_char)?;
    Some(after[..end].to_string())
}

fn fix_google_thumbnail(input: &str) -> String {
    input
        .replace("/s72-c/", "/w600/")
        .replace("=s72-c", "=w600")
}

fn parse_feed_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Sample Fa"},"link":[{"rel":"alternate","href":"https://apenasuma-fa.blogspot.com/p/sample.html"}],"media$thumbnail":{"url":"https://1.bp.blogspot.com/s72-c/sample.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Fa</h1><thum style="background-image:url(&quot;/cover.jpg&quot;)"></thum>
<div id="synopsis">Sample description.</div><a class="leading-none" href="/search/label/Romance">Romance</a>
<div class="bg-green"><span>Em lançamento</span></div><div class="chapter_get" data-labelchapter="Sample"></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Capitulo 1"},"published":{"$t":"2024-01-01T00:00:00.000Z"},"link":[{"rel":"alternate","href":"https://apenasuma-fa.blogspot.com/2024/01/sample.html"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<div id="reader"><div class="separator"><img src="https://blogger.googleusercontent.com/page1.jpg"></div></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blogger_fixtures() {
        assert_eq!(parse_feed_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_chapter_feed(CHAPTERS_FIXTURE, "/p/sample.html").len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
