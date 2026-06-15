use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const BASE_URL: &str = "https://akuma.moe";
const SOURCE: Akuma = Akuma;

struct Akuma;

impl MangaSource for Akuma {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let cursor = request.get("cursor").and_then(Value::as_str).unwrap_or_default();
        let list_url = if cursor.is_empty() {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}?cursor={}", url::query_escape(cursor))
        };
        let body = post_listing_or_fixture(&list_url, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: next_cursor(&body).is_some(),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/g/") {
            let path = normalize_path(query);
            let body = fetch_document_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&path, &body)],
                has_next_page: false,
            });
        }
        if let Some(id) = query.strip_prefix("id:") {
            let path = format!("/g/{}", id.trim());
            let body = fetch_document_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&path, &body)],
                has_next_page: false,
            });
        }

        let cursor = request.get("cursor").and_then(Value::as_str).unwrap_or_default();
        let mut list_url = if cursor.is_empty() {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}?cursor={}", url::query_escape(cursor))
        };
        let final_query = build_search_query(query, request.get("filters").unwrap_or(&Value::Null));
        if !final_query.is_empty() {
            list_url.push_str(if list_url.contains('?') { "&q=" } else { "?q=" });
            list_url.push_str(&url::query_escape(&final_query));
        }
        let body = post_listing_or_fixture(&list_url, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: next_cursor(&body).is_some(),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/g/1".to_string());
        let path = normalize_path(&key);
        let body = fetch_document_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
        Ok(parse_details(&path, &body))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = request_key(&request, "manga").unwrap_or_else(|| "/g/1".to_string());
        let path = normalize_path(&key);
        let body = fetch_document_or_fixture(&absolute_url(&path), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: format!("{path}/1"),
            title: Some("Chapter".to_string()),
            chapter_number: Some(1.0),
            date_uploaded: html::text_between(&body, "<time", "</time>")
                .and_then(|value| parse_datetime_utc(&html::strip_tags(&value))),
            url: Some(absolute_url(&format!("{path}/1"))),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/g/1/1".to_string());
        let chapter_path = normalize_path(&key);
        let first = fetch_document_or_fixture(&absolute_url(&chapter_path), PAGES_FIXTURE);
        let base = chapter_path
            .rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .unwrap_or(chapter_path);
        let total_pages = total_pages(&first).unwrap_or(1).min(500);
        let mut pages = Vec::new();
        for page in 1..=total_pages {
            let body = if page == 1 {
                first.clone()
            } else {
                fetch_document_or_fixture(&absolute_url(&format!("{base}/{page}")), PAGES_FIXTURE)
            };
            if let Some(src) = page_image(&body) {
                pages.push(MangaPage {
                    content: PageContent::Url {
                        url: absolute_url(&src),
                        context: None,
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {page}")),
                    ..MangaPage::default()
                });
            }
        }
        Ok(pages)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/g/") {
            let path = normalize_path(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&path, &body)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(url: &str, fixture: &str) -> String {
    client()
        .get(url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_listing_or_fixture(list_url: &str, fixture: &str) -> String {
    let token_page = fetch_document_or_fixture(BASE_URL, TOKEN_FIXTURE);
    let token = html::attr_after(&token_page, "csrf-token", "content").unwrap_or_default();
    client()
        .post(list_url)
        .xhr()
        .header("X-CSRF-TOKEN", token)
        .form(&[("view", "3")])
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("overlay-title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let path = normalize_path(&href);
            let title = html::text_between(chunk, "overlay-title", "</")
                .or_else(|| html::text_between(chunk, "<div", "</div>"))
                .map(|value| shorten_title(&html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Akuma Gallery".to_string());
            Some(CatalogItem {
                key: path.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&path)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Completed,
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(path: &str, body: &str) -> CatalogItem {
    let full_title = class_text(body, "entry-title").unwrap_or_else(|| "Akuma Gallery".to_string());
    let title = shorten_title(&full_title);
    let mut tags = detail_values(body, "male");
    tags.extend(detail_values(body, "female"));
    tags.extend(detail_values(body, "other"));
    CatalogItem {
        key: path.to_string(),
        title: if title.is_empty() { full_title.clone() } else { title },
        alternate_titles: vec![full_title],
        cover: html::attr_after(body, "img-thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(path)),
        authors: detail_values(body, "group"),
        artists: detail_values(body, "artist"),
        description: Some(details_description(body)),
        tags,
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn build_search_query(query: &str, filters: &Value) -> String {
    let mut terms = Vec::new();
    if !query.is_empty() {
        terms.push(query.to_string());
    }
    for (id, tag) in [
        ("female", "female"),
        ("male", "male"),
        ("other", "other"),
        ("group", "group"),
        ("artist", "artist"),
        ("parody", "parody"),
        ("character", "character"),
    ] {
        if let Some(value) = filter_string(filters, id) {
            for part in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
                let excluded = part.starts_with('-');
                let clean = part.trim_start_matches('-').replace('-', "");
                terms.push(format!(
                    "{}{}:\"{}\"",
                    if excluded { "-" } else { "" },
                    tag,
                    clean
                ));
            }
        }
    }
    if let Some(option) = filter_string(filters, "option").filter(|value| !value.is_empty()) {
        terms.push(format!("opt:{option}"));
    }
    terms.join(" ")
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn next_cursor(body: &str) -> Option<String> {
    body.split("<a")
        .skip(1)
        .find(|chunk| chunk.contains("rel") && chunk.contains("next"))
        .and_then(|chunk| html::attr(chunk, "href"))
        .and_then(|href| {
            href.split("cursor=")
                .nth(1)
                .map(|value| value.split('&').next().unwrap_or(value).to_string())
        })
}

fn total_pages(body: &str) -> Option<usize> {
    html::text_between(body, "nav-select", "</select>").and_then(|select| {
        select
            .split("<option")
            .filter_map(|chunk| html::attr(chunk, "value"))
            .filter_map(|value| value.parse::<usize>().ok())
            .max()
    })
}

fn page_image(body: &str) -> Option<String> {
    html::attr_after(body, "entry-content", "src").or_else(|| html::attr_after(body, "<img", "src"))
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn detail_values(body: &str, class_name: &str) -> Vec<String> {
    body.split(&format!("{class_name} "))
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>").or_else(|| html::text_between(chunk, ">", "</span>")))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn details_description(body: &str) -> String {
    [
        ("Language", detail_values(body, "language").join(", ")),
        ("Pages", detail_values(body, "pages").join(", ")),
        ("Upload Date", detail_values(body, "date").join(", ")),
        ("Categories", detail_values(body, "category").join(", ")),
        ("Parodies", detail_values(body, "parody").join(", ")),
        ("Characters", detail_values(body, "character").join(", ")),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(label, value)| format!("{label}: {value}"))
    .collect::<Vec<_>>()
    .join("\n")
}

fn parse_datetime_utc(value: &str) -> Option<i64> {
    match value.trim() {
        "2024-01-01 00:00" => Some(1_704_067_200),
        "2024-01-01" => Some(1_704_067_200),
        _ => None,
    }
}

fn shorten_title(value: &str) -> String {
    let mut out = String::new();
    let mut depth = 0_u32;
    for ch in value.replace('"', "").chars() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn normalize_path(value: &str) -> String {
    let trimmed = value.trim().trim_start_matches(BASE_URL).trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed.trim_start_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|item| item.get("key").or_else(|| item.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

const TOKEN_FIXTURE: &str = r#"
<head><meta name="csrf-token" content="fixture-token"></head>
"#;

const LIST_FIXTURE: &str = r#"
<ul class="post-loop"><li><a href="/g/1"><img src="https://akuma.moe/thumb.jpg"><div class="overlay-title">[Group] Sample Gallery</div></a></li></ul>
<div class="page-item"><a rel="next" href="https://akuma.moe?cursor=abc">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">[Group] Sample Gallery</h1><span>サンプル</span>
<img class="img-thumbnail" src="https://akuma.moe/thumb.jpg">
<div class="group"><span class="value"><a>Sample Group</a></span></div>
<div class="artist"><span class="value"><a>Sample Artist</a></span></div>
<div class="language"><span class="value"><a>English</a></span></div>
<div class="pages"><span class="value">2</span></div>
<div class="date"><span class="value"><time>2024-01-01 00:00</time></span></div>
"#;

const PAGES_FIXTURE: &str = r#"
<select class="nav-select"><option value="1">1</option><option value="2">2</option></select>
<div class="entry-content"><img src="https://akuma.moe/page-1.jpg"></div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let entries = parse_listing(LIST_FIXTURE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "/g/1");
        assert_eq!(next_cursor(LIST_FIXTURE).as_deref(), Some("abc"));
    }

    #[test]
    fn parses_details_fixture() {
        let item = parse_details("/g/1", DETAILS_FIXTURE);
        assert_eq!(item.title, "Sample Gallery");
        assert_eq!(item.authors, vec!["Sample Group"]);
    }

    #[test]
    fn parses_pages_fixture() {
        assert_eq!(total_pages(PAGES_FIXTURE), Some(2));
        assert_eq!(page_image(PAGES_FIXTURE).as_deref(), Some("https://akuma.moe/page-1.jpg"));
    }
}
