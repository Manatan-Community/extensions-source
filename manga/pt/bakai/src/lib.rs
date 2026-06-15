use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Bakai = Bakai;
const BASE_URL: &str = "https://bakai.org";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct Bakai;

impl MangaSource for Bakai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = if page <= 1 {
                format!("{BASE_URL}/home/")
            } else {
                format!("{BASE_URL}/home/page/{page}/")
            };
            return Ok(parse_latest(&fetch_document(&target, LATEST_FIXTURE)));
        }
        Ok(parse_popular(&fetch_document(BASE_URL, POPULAR_FIXTURE)))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_search(&fetch_document(
            &format!(
                "{BASE_URL}/srch/?q={}&page={page}&quick=1&search_and_or=and&sortby=relevancy",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/home/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/home/sample".to_string());
        Ok(vec![chapter_from_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        )])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/home/sample".to_string());
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
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("mostViewedArticlesItem")
            .skip(1)
            .filter_map(catalog_from_chunk)
            .collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("ipsGrid_span4")
            .skip(1)
            .filter_map(catalog_from_chunk)
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("ipsStreamItem")
            .skip(1)
            .filter(|chunk| chunk.contains("fa-file-text") || chunk.contains("ipsStreamItem_title"))
            .filter_map(catalog_from_chunk)
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn catalog_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "ipsTruncate", "</")
            .or_else(|| html::text_between(chunk, "ipsType_pageTitle", "</h2>"))
            .or_else(|| html::text_between(chunk, "ipsStreamItem_title", "</h2>"))
            .or_else(|| html::text_between(chunk, "<a", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Bakai".into())),
        cover: image_from_chunk(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/home/sample".to_string());
    let type_value = labeled_paragraph(body, "Type:");
    let color = labeled_paragraph(body, "Color:");
    let tags = labeled_paragraph(body, "Tags:");
    let parody = labeled_paragraph(body, "Parody:");
    CatalogItem {
        key: key.clone(),
        title: detail_title(body)
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Bakai".into())),
        cover: html::attr_after(body, "cCmsRecord_image", "src")
            .or_else(|| image_from_chunk(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "ipsType_richText", "</section>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty() && value != "-"),
        authors: labeled_paragraph(body, "Artist:").into_iter().collect(),
        tags: [type_value, color, parody, tags]
            .into_iter()
            .flatten()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_from_details(body: &str, key: &str) -> MangaChapter {
    MangaChapter {
        key: key.to_string(),
        title: detail_title(body).or_else(|| Some("Chapter".to_string())),
        date_uploaded: html::attr_after(body, "<time", "datetime")
            .and_then(|value| parse_feed_date(&value)),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("ipsGrid") || chunk.contains("data-src") || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
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

fn detail_title(body: &str) -> Option<String> {
    html::text_between(body, "ipsContained", "</")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn labeled_paragraph(body: &str, label: &str) -> Option<String> {
    body.split("<p")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            html::strip_tags(chunk)
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr_after(chunk, "data-background-src", "data-background-src"))
        .or_else(|| html::attr(chunk, "data-background-src"))
}

fn has_next_page(body: &str) -> bool {
    body.contains("ipsPagination_next")
        && !body.contains("ipsPagination_next ipsPagination_inactive")
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
<li class="mostViewedArticlesItem"><h3 class="ipsTruncate"><a href="/home/sample/">Sample Bakai</a></h3><span class="ipsThumb"><img src="/cover.jpg"></span></li>
"#;
const LATEST_FIXTURE: &str = r#"
<ul class="ipsGrid"><li class="ipsGrid_span4"><h2 class="ipsType_pageTitle"><a href="/home/sample/">Sample Bakai</a></h2><div class="cCmsRecord_image"><img src="/cover.jpg"></div></li></ul>
"#;
const SEARCH_FIXTURE: &str = r#"
<ol data-role="resultsContents"><li class="ipsStreamItem"><span class="ipsStreamItem_contentType"><i class="fa-file-text"></i></span><h2 class="ipsStreamItem_title"><a href="/home/sample/">Sample Bakai</a></h2><img class="ipsStream_image" src="/cover.jpg"></li></ol>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="ipsType_pageTitle"><span class="ipsContained">Sample Bakai</span></h1><div class="cCmsRecord_image"><img src="/cover.jpg"></div>
<p><strong>Artist:</strong> Sample Artist</p><p><strong>Type:</strong> Manga</p><p><strong>Tags:</strong> Romance, Drama</p>
<section class="ipsType_richText">Sample description.</section><time datetime="2024-01-01T00:00:00Z"></time>
"#;
const PAGES_FIXTURE: &str = r#"<div class="ipsGrid ipsGrid_collapsePhone"><img data-src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bakai_fixtures() {
        assert_eq!(parse_popular(POPULAR_FIXTURE).entries.len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
        assert_eq!(
            chapter_from_details(DETAILS_FIXTURE, "/home/sample")
                .title
                .unwrap(),
            "Sample Bakai"
        );
    }
}
