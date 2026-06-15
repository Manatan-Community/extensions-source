use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Bacami = Bacami;
const BASE_URL: &str = "https://bacami.net";

struct Bacami;

impl MangaSource for Bacami {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "score"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/custom-search/orderby/{order}/page/{page}/"),
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
        let target = if query.is_empty() {
            search_url(page, request.get("filters"))
        } else {
            format!(
                "{BASE_URL}/search/{}/page/{page}/",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample-bacami".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/komik/sample-bacami".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/komik/sample-bacami/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn search_url(page: u64, filters: Option<&Value>) -> String {
    if filters
        .and_then(|value| value.get("new"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return format!("{BASE_URL}/komik-baru/");
    }
    let orderby = filter(filters, "orderby").unwrap_or("latest");
    let status = filter(filters, "status").unwrap_or("all");
    let media_type = filter(filters, "type").unwrap_or("all");
    let genre = filter(filters, "genre").unwrap_or("all");
    let mut target = format!("{BASE_URL}/custom-search/");
    if orderby != "latest" {
        target.push_str(&format!("orderby/{orderby}/"));
    }
    if status != "all" {
        target.push_str(&format!("status/{status}/"));
    }
    if media_type != "all" {
        target.push_str(&format!("type/{media_type}/"));
    }
    if genre != "all" {
        target.push_str(&format!("genre/{genre}/"));
    }
    target.push_str(&format!("page/{page}/"));
    target
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("genre-card")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "genre-cover", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "genre-info", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Bacami".into());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/komik/sample-bacami".to_string());
    let mut description = html::text_between(body, "manga-description", "</p>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(alt) = html::text_between(body, "manga-altname", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        description = format!("{description}\n\nAlternative Title: {alt}")
            .trim()
            .to_string();
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Bacami".to_string()),
        cover: html::attr_after(body, "image-wrap", "data-src")
            .or_else(|| html::attr_after(body, "image-wrap", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: (!description.is_empty()).then_some(description),
        authors: info_value(body, "Author").into_iter().collect(),
        tags: body
            .split("<nav")
            .nth(1)
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("ch-link"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "ch-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let raw_title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Chapter".to_string());
            let title = raw_title
                .split_once(['-', '–'])
                .map(|(_, value)| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or(raw_title);
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "ch-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_day_month_year(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    image_urls_from_script(body)
        .into_iter()
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

fn image_urls_from_script(body: &str) -> Vec<String> {
    let Some(start) = body.find("imageUrls:") else {
        return Vec::new();
    };
    let rest = &body[start + "imageUrls:".len()..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };
    serde_json::from_str(&rest[open..=open + close]).unwrap_or_default()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("info-item")
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .and_then(|chunk| html::text_between(chunk, "info-value", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(body: &str) -> ItemStatus {
    if body.contains("tamat-tag") {
        ItemStatus::Completed
    } else if body.contains("hot-tag") || body.contains("project-tag") {
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

fn parse_day_month_year(value: &str) -> Option<i64> {
    let parts = value.trim().replace(',', "");
    let mut iter = parts.split_whitespace();
    let day = iter.next()?.parse::<u32>().ok()?;
    let month = month_number(iter.next()?)?;
    let year = iter.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn month_number(value: &str) -> Option<u32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sep" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        "december" | "dec" => 12,
        _ => return None,
    })
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
<article class="genre-card"><div class="genre-cover"><a href="/komik/sample-bacami/"><img data-src="/cover.jpg"></a></div><div class="genre-info"><a>Sample Bacami</a></div></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<div id="komik"><section class="manga-content"><header><h1>Sample Bacami</h1></header><figure><div class="image-wrap"><img src="/cover.jpg"></div></figure>
<div class="info-item"><span>Author</span><span class="info-value">Writer</span></div>
<nav><span><a>Action</a></span></nav><p class="manga-description">Sample description.</p><p class="manga-altname">Alt Sample</p><span class="hot-tag"></span></section></div>
<ol class="chapter-list"><li><a class="ch-link" href="/komik/sample-bacami/chapter-1">Sample - Chapter 1</a><span class="ch-date">01 January, 2024</span></li></ol>
"#;

const PAGES_FIXTURE: &str = r#"<script>window.reader = { imageUrls: ["https://bacami.net/page1.jpg","/page2.jpg"], other: true };</script>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bacami_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Bacami"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE)[0].chapter_number, Some(1.0));
        assert_eq!(image_urls_from_script(PAGES_FIXTURE).len(), 2);
    }
}
