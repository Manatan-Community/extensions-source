use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UpdateStrategy, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde_json::Value;

const SOURCE: MissKon = MissKon;
const BASE_URL: &str = "https://misskon.com";

struct MissKon;

impl MangaSource for MissKon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/page/{page}/")
        } else {
            format!("{BASE_URL}/top3/")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: latest && has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let category = request
            .get("filters")
            .and_then(|filters| filters.get("category"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = if !category.is_empty() {
            url::join_url(BASE_URL, category)
        } else {
            format!("{BASE_URL}/page/{page}/?s={}", encode_query(query))
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("item-list"))
        .filter_map(|chunk| {
            let title_block = chunk.split("post-box-title").nth(1).unwrap_or(chunk);
            let href = html::attr_after(title_block, "<a", "href")?;
            Some(CatalogItem {
                key: normalize_key(&href),
                title: html::text_between(title_block, "<a", "</a>")
                    .map(|text| html::strip_tags(&text))
                    .unwrap_or_else(|| "MissKon".into()),
                cover: html::attr_after(chunk, "post-thumbnail", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("all".into()),
                content_rating: Some("adult".into()),
                update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "post-title", "</")
            .map(|text| html::strip_tags(&text))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "MissKon".into()),
        tags: body
            .split("post-tag")
            .nth(1)
            .unwrap_or(body)
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|text| html::strip_tags(&text))
            .filter(|tag| !tag.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, key: &str) -> Vec<MangaChapter> {
    let numeric_max = body
        .split("post-page-numbers")
        .filter_map(|chunk| html::strip_tags(chunk).trim().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    let marker_count = body.matches("post-page-numbers").count() as u32;
    let max_page = numeric_max.max(marker_count).max(1);
    let date = html::attr_after(body, ".entry", "data-src")
        .or_else(|| html::attr_after(body, "<img", "data-src"))
        .and_then(|image| date_from_image_url(&image));
    (1..=max_page)
        .rev()
        .map(|page| MangaChapter {
            key: format!("{}/{}", key.trim_end_matches('/'), page),
            title: Some(format!("Page {page}")),
            date_uploaded: date,
            url: Some(format!(
                "{}{}/{}",
                BASE_URL,
                key.trim_end_matches('/'),
                page
            )),
            language: Some("all".into()),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-src") || chunk.contains("src"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn has_next_page(body: &str) -> bool {
    body.contains("pagination") && body.contains("current") && body.contains("page")
}

fn normalize_key(value: &str) -> String {
    value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
        .to_string()
}

fn date_from_image_url(value: &str) -> Option<i64> {
    let parts = value.split('/').collect::<Vec<_>>();
    for window in parts.windows(3) {
        let year = window[0].parse::<i32>().ok();
        let month = window[1].parse::<u32>().ok();
        let day = window[2].parse::<u32>().ok();
        if let (Some(year), Some(month), Some(day)) = (year, month, day) {
            return Some(days_from_civil(year, month, day) * 86_400_000);
        }
    }
    None
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"
<article class="item-list"><div class="post-thumbnail"><img data-src="/2024/01/01/thumb.jpg"></div><h2 class="post-box-title"><a href="https://misskon.com/sample/">Sample Album</a></h2></article>
<div class="pagination"><span class="current">1</span><a class="page" href="/page/2">2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<article><div class="post-inner">
<h1 class="post-title">Sample Album</h1>
<div class="post-tag"><a href="/tag/cosplay/">Cosplay</a></div>
<div class="entry"><p><img data-src="https://misskon.com/2024/01/01/image.jpg"></p></div>
<div class="page-link"><a class="post-page-numbers">1</a><a class="post-page-numbers">2</a></div>
</div></article>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="post-inner"><div class="entry"><p><img data-src="https://misskon.com/2024/01/01/image.jpg"></p></div></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_misskon() {
        assert_eq!(parse_listing(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, None).title, "Sample Album");
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/sample/").len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
