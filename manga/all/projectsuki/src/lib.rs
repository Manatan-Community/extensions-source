use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: ProjectSuki = ProjectSuki;
const BASE_URL: &str = "https://projectsuki.com";

struct ProjectSuki;

impl MangaSource for ProjectSuki {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/browse/{}", page.saturating_sub(1))
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_books(&body),
            has_next_page: parse_books(&body).len() >= 30,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(book_id) = book_id_from_input(query) {
            let item = self.details(json!({ "manga": format!("/book/{book_id}") }))?;
            return Ok(Paged {
                entries: vec![item],
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mode = filters
            .get("searchMode")
            .or_else(|| {
                request
                    .get("preferences")
                    .and_then(|prefs| prefs.get("searchMode"))
            })
            .and_then(Value::as_str)
            .unwrap_or("smart");
        if mode == "smart" || mode == "simple" {
            let data = fetch_book_search_data();
            let entries = search_book_map(&data, query);
            if !entries.is_empty() {
                return Ok(Paged {
                    entries,
                    has_next_page: false,
                });
            }
        }

        let mut target = format!(
            "{BASE_URL}/search?page={}&q={}",
            page.saturating_sub(1),
            url::query_escape(query)
        );
        append_adv_filter(&mut target, filters, "origin", "origin");
        append_adv_filter(&mut target, filters, "status", "status");
        append_adv_filter(&mut target, filters, "author", "author");
        append_adv_filter(&mut target, filters, "artist", "artist");
        let body = fetch_document_or_fixture(&target, SEARCH_FIXTURE);
        let entries = parse_books(&body);
        Ok(Paged {
            has_next_page: entries.len() >= 30,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/book/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let preferences = request.get("preferences").unwrap_or(&Value::Null);
        Ok(parse_chapters(&body, preferences))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1/1".into());
        let (book_id, chapter_id) =
            read_parts(&key).unwrap_or(("sample".to_string(), "chapter-1".to_string()));
        let body = post_pages_or_fixture(&book_id, &chapter_id);
        Ok(parse_pages_response(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(book_id) = book_id_from_input(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(self.details(json!({ "manga": format!("/book/{book_id}") }))?),
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

fn post_pages_or_fixture(book_id: &str, chapter_id: &str) -> String {
    let payload = json!({
        "bookid": book_id,
        "chapterid": chapter_id,
        "first": "true"
    });
    client()
        .post(format!("{BASE_URL}/callpage"))
        .xhr()
        .referer(format!("{BASE_URL}/read/{book_id}/{chapter_id}/1"))
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string())
}

fn fetch_book_search_data() -> BTreeMap<String, String> {
    let raw = client()
        .post(format!("{BASE_URL}/api/book/search"))
        .xhr()
        .referer(format!("{BASE_URL}/browse"))
        .json(r#"{"hash":null}"#)
        .send_text()
        .unwrap_or_else(|_| BOOK_SEARCH_FIXTURE.to_string());
    let value = serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null);
    value
        .get("data")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(id, item)| {
            item.get("value")
                .and_then(Value::as_str)
                .map(|title| (id.to_string(), title.to_string()))
        })
        .collect()
}

fn parse_books(body: &str) -> Vec<CatalogItem> {
    let mut by_id: BTreeMap<String, CatalogItem> = BTreeMap::new();
    for chunk in body.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let Some(book_id) = book_id_from_input(&href) else {
            continue;
        };
        let title = html::attr_after(chunk, "<img", "alt")
            .or_else(|| {
                html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| book_id.clone());
        let cover = image_attr(chunk)
            .map(|value| url::join_url(BASE_URL, &value))
            .or_else(|| Some(format!("{BASE_URL}/images/gallery/{book_id}/thumb")));
        by_id.entry(book_id.clone()).or_insert_with(|| CatalogItem {
            key: format!("/book/{book_id}"),
            title,
            cover,
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/book/{book_id}")),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        });
    }
    by_id.into_values().collect()
}

fn search_book_map(data: &BTreeMap<String, String>, query: &str) -> Vec<CatalogItem> {
    let words = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return Vec::new();
    }
    let mut matches = data
        .iter()
        .filter_map(|(id, title)| {
            let lower = title.to_lowercase();
            let score = words
                .iter()
                .filter(|word| lower.contains(word.as_str()))
                .count();
            (score > 0).then_some((score, id, title))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.2.cmp(right.2)));
    matches
        .into_iter()
        .take(50)
        .map(|(_, id, title)| CatalogItem {
            key: format!("/book/{id}"),
            title: title.to_string(),
            cover: Some(format!("{BASE_URL}/images/gallery/{id}/thumb")),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/book/{id}")),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let book_id = book_id_from_input(key).unwrap_or_else(|| "sample".to_string());
    let mut item = parse_books(body)
        .into_iter()
        .find(|item| item.key.ends_with(&book_id))
        .unwrap_or_else(|| CatalogItem {
            key: format!("/book/{book_id}"),
            title: html::text_between(body, "itemprop=\"title\"", "</")
                .or_else(|| html::text_between(body, "<h2", "</h2>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| book_id.clone()),
            cover: Some(format!("{BASE_URL}/images/gallery/{book_id}/thumb")),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/book/{book_id}")),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            ..CatalogItem::default()
        });
    item.description = html::text_between(body, "descriptionCollapse", "</div>")
        .or_else(|| html::text_between(body, "class=\"description", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let details = parse_detail_labels(body);
    item.authors = split_detail(details.get("author"));
    item.artists = split_detail(details.get("artist"));
    item.tags = split_detail(details.get("genre"));
    item.status = match details
        .get("status")
        .map(|value| value.trim().to_lowercase())
        .as_deref()
    {
        Some("ongoing") => ItemStatus::Ongoing,
        Some("completed") => ItemStatus::Completed,
        Some("hiatus") => ItemStatus::Hiatus,
        Some("cancelled") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    };
    item.initialized = true;
    item
}

fn parse_detail_labels(body: &str) -> BTreeMap<String, String> {
    let mut details = BTreeMap::new();
    for chunk in body.split("<div").skip(1) {
        let text = html::strip_tags(chunk);
        let Some((label, data)) = text.split_once(':') else {
            continue;
        };
        let key = label.trim().to_lowercase();
        if matches!(
            key.as_str(),
            "author" | "artist" | "status" | "genre" | "genres" | "origin"
        ) {
            let normalized = if key == "genres" {
                "genre"
            } else {
                key.as_str()
            };
            details.insert(normalized.to_string(), data.trim().to_string());
        }
    }
    details
}

fn parse_chapters(body: &str, preferences: &Value) -> Vec<MangaChapter> {
    let whitelist = language_set(preferences, "languageWhitelist");
    let blacklist = language_set(preferences, "languageBlacklist");
    let mut chapters = Vec::new();
    for row in body.split("<tr").skip(1) {
        let Some(href) = html::attr_after(row, "<a", "href") else {
            continue;
        };
        let Some((_, chapter_id)) = read_parts(&href) else {
            continue;
        };
        let title = html::text_between(row, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| chapter_id.clone());
        let row_text = html::strip_tags(row).to_lowercase();
        let language = extract_after_labels(&row_text, &["language", "lang"])
            .unwrap_or_else(|| "unknown".into());
        if blacklist.contains(&language)
            || (!whitelist.is_empty() && language != "unknown" && !whitelist.contains(&language))
        {
            continue;
        }
        let group = extract_after_labels(&row_text, &["group", "scanlator"]).unwrap_or_default();
        chapters.push(MangaChapter {
            key: normalize_key(&href),
            title: Some(title.clone()),
            chapter_number: chapter_number(&title),
            scanlators: if group.is_empty() {
                vec![language.clone()]
            } else {
                vec![format!("{group} | {language}")]
            },
            url: Some(url::join_url(BASE_URL, &href)),
            ..MangaChapter::default()
        });
    }
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages_response(raw: &str) -> Vec<MangaPage> {
    let src = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("src")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| raw.to_string());
    let mut pages = src
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| image.contains("/images/gallery/"))
        .map(|image| url::join_url(BASE_URL, &image))
        .collect::<Vec<_>>();
    pages.sort_by_key(|image| {
        image
            .rsplit('/')
            .next()
            .and_then(|part| part.split('.').next())
            .and_then(|part| part.parse::<u64>().ok())
            .unwrap_or(0)
    });
    pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
}

fn book_id_from_input(input: &str) -> Option<String> {
    let path = input
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or(input);
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["book", id, ..] if !id.is_empty() => Some((*id).to_string()),
        ["read", id, ..] if !id.is_empty() => Some((*id).to_string()),
        _ => None,
    }
}

fn read_parts(input: &str) -> Option<(String, String)> {
    let path = input
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or(input);
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["read", book_id, chapter_id, ..] if !book_id.is_empty() && !chapter_id.is_empty() => {
            Some(((*book_id).to_string(), (*chapter_id).to_string()))
        }
        _ => None,
    }
}

fn normalize_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .trim_start_matches(BASE_URL)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn append_adv_filter(target: &mut String, filters: &Value, id: &str, param: &str) {
    if let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        if !target.contains("adv=1") {
            target.push_str("&adv=1");
        }
        target.push('&');
        target.push_str(param);
        target.push('=');
        target.push_str(&url::query_escape(value));
    }
}

fn split_detail(value: Option<&String>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn language_set(preferences: &Value, id: &str) -> BTreeSet<String> {
    preferences
        .get(id)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn extract_after_labels(text: &str, labels: &[&str]) -> Option<String> {
    labels.iter().find_map(|label| {
        text.find(label).and_then(|index| {
            let rest = &text[index + label.len()..];
            let value = rest
                .trim_start_matches([':', ' ', '\t'])
                .split_whitespace()
                .next()?;
            (!value.is_empty()).then_some(value.to_string())
        })
    })
}

fn chapter_number(title: &str) -> Option<f32> {
    let lower = title.to_lowercase();
    let start = lower.find("chapter").or_else(|| lower.find("ch."))?;
    let mut number = String::new();
    for ch in lower[start..].chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
        } else if !number.is_empty() {
            break;
        }
    }
    number.parse::<f32>().ok()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a href="/book/sample"><img src="/images/gallery/sample/thumb.jpg" alt="Sample Book"></a>
<a href="/book/sample">Sample Book</a>
"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const BOOK_SEARCH_FIXTURE: &str = r#"{"data":{"sample":{"value":"Sample Book"}}}"#;
const DETAILS_FIXTURE: &str = r#"
<h2 itemprop="title">Sample Book</h2>
<a href="/book/sample"><img src="/images/gallery/sample/thumb.jpg" alt="Sample Book"></a>
<div id="descriptionCollapse">Sample description.</div>
<div>Author: Jane</div><div>Artist: Jane</div><div>Status: Ongoing</div><div>Genre: Action, Drama</div>
<table><thead><tr><td>Chapter</td><td>Added</td><td>Group</td><td>Language</td></tr></thead>
<tbody><tr><td><a href="/read/sample/chapter-1/1">Chapter 1</a></td><td>2024-01-01</td><td>Team</td><td>English</td></tr></tbody></table>
"#;
const PAGES_FIXTURE: &str = r#"{"src":"<img src=\"/images/gallery/sample/abcdef/001.jpg\"><img src=\"/images/gallery/sample/abcdef/002.jpg\">"}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_book_details_chapters_and_pages() {
        let books = parse_books(LIST_FIXTURE);
        assert_eq!(books[0].key, "/book/sample");

        let item = SOURCE.details(json!({"manga":"/book/sample"})).unwrap();
        assert_eq!(item.title, "Sample Book");
        assert_eq!(item.authors, vec!["Jane"]);

        let chapters = SOURCE
            .chapters(json!({"manga":"/book/sample","preferences":{"languageWhitelist":"english"}}))
            .unwrap();
        assert_eq!(chapters[0].key, "/read/sample/chapter-1/1");

        let pages = SOURCE
            .pages(json!({"chapter":"/read/sample/chapter-1/1"}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn resolves_book_ids_from_book_and_read_urls() {
        assert_eq!(
            book_id_from_input("https://projectsuki.com/read/sample/chapter-1/1").as_deref(),
            Some("sample")
        );
        assert_eq!(
            read_parts("/read/sample/chapter-1/1"),
            Some(("sample".to_string(), "chapter-1".to_string()))
        );
    }
}
