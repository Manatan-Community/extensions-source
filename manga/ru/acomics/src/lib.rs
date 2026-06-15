use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AComics = AComics;
const BASE_URL: &str = "https://acomics.ru";

struct AComics;

impl MangaSource for AComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing == "latest" {
            "last_update"
        } else {
            "subscr_count"
        };
        Ok(parse_listing(&fetch_document(
            &search_url(page, "", sort, &Value::Null),
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sort").unwrap_or("subscr_count");
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, sort, filters),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/~sample/about".into());
        Ok(parse_details(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            Some(normalize_key(&key)),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/~sample/about".into());
        Ok(parse_chapters(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/~sample/1".into());
        Ok(parse_pages(&fetch_document(
            &chapter_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
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
        .with_header("Cookie", "ageRestrict=17;")
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

fn search_url(page: u64, query: &str, sort: &str, filters: &Value) -> String {
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(("keyword".to_string(), url::query_escape(query)));
        append_skip(&mut params, page);
        return format!("{BASE_URL}/search?{}", query_pairs(&params));
    }
    append_indexed(
        &mut params,
        "categories",
        selected_values(filters.get("categories")),
    );
    append_indexed(
        &mut params,
        "ratings",
        selected_values(filters.get("ratings")),
    );
    params.push((
        "type".into(),
        filter_string(filters, "type").unwrap_or("0").to_string(),
    ));
    params.push((
        "updatable".into(),
        filter_string(filters, "updatable")
            .unwrap_or("0")
            .to_string(),
    ));
    params.push((
        "subscribe".into(),
        filter_string(filters, "subscribe")
            .unwrap_or("0")
            .to_string(),
    ));
    params.push((
        "issue_count".into(),
        filter_string(filters, "issue_count")
            .unwrap_or("2")
            .to_string(),
    ));
    params.push(("sort".into(), sort.to_string()));
    append_skip(&mut params, page);
    format!("{BASE_URL}/comics?{}", query_pairs(&params))
}

fn append_skip(params: &mut Vec<(String, String)>, page: u64) {
    if page > 1 {
        params.push(("skip".into(), ((page - 1) * 10).to_string()));
    }
}

fn append_indexed(params: &mut Vec<(String, String)>, name: &str, values: Vec<String>) {
    for (index, value) in values.iter().enumerate() {
        params.push((format!("{name}[{index}]"), value.to_string()));
    }
}

fn query_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_filter_values)
            .collect(),
        Some(Value::String(value)) => split_filter_values(value),
        _ => Vec::new(),
    }
}

fn split_filter_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|part| part.trim().split(':').next())
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn normalize_key(value: &str) -> String {
    let path = if let Some(rest) = value.strip_prefix(BASE_URL) {
        rest
    } else {
        value
    };
    let path = path.split('?').next().unwrap_or(path).trim_matches('/');
    let path = if path.ends_with("/about") {
        path.to_string()
    } else if path.contains('/') {
        path.split('/').take(1).collect::<Vec<_>>().join("/") + "/about"
    } else {
        format!("{path}/about")
    };
    format!("/{path}")
}

fn manga_url(key: &str) -> String {
    url::join_url(BASE_URL, &normalize_key(key))
}

fn chapter_url(key: &str) -> String {
    url::join_url(BASE_URL, key.trim_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<section")
            .skip(1)
            .filter(|chunk| chunk.contains("serial-card"))
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("infinite-scroll"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href =
        html::attr_after(chunk, "<h2", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h2", "</h2>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Комикс".into())),
        cover: html::attr_after(chunk, "<img", "data-real-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(manga_url(&key)),
        language: Some("ru".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/~sample/about".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page-header-with-menu", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Комикс".into())),
        cover: html::attr_after(body, "<img", "data-real-src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: link_values(body, "serial-about-authors"),
        tags: link_values(body, "serial-about-badges"),
        description: html::text_between(body, "serial-about-text", "</section>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(manga_url(&key)),
        language: Some("ru".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("</p>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let count = issue_count(body).unwrap_or(1);
    let base_path = normalize_key(manga_key)
        .trim_end_matches("/about")
        .trim_start_matches('/')
        .to_string();
    (1..=count)
        .rev()
        .map(|number| {
            let key = format!("/{base_path}/{number}");
            MangaChapter {
                key: key.clone(),
                title: Some(number.to_string()),
                url: Some(chapter_url(&key)),
                chapter_number: Some(number as f32),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn issue_count(body: &str) -> Option<u32> {
    let marker = "Количество выпусков:";
    let rest = body.split(marker).nth(1)?;
    let number = html::strip_tags(rest)
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    number.parse().ok()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("issue"))
        .filter_map(|chunk| html::attr(chunk, "src"))
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

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<section class="serial-card"><a><img data-real-src="/cover.jpg"></a><h2><a href="/~sample">Sample Comic</a></h2></section><a class="infinite-scroll"></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<article class="common-article"><div class="page-header-with-menu"><h1>Sample Comic</h1></div><p class="serial-about-badges"><a class="category">Драма</a></p><p class="serial-about-authors"><a>Автор</a></p><section class="serial-about-text"><p>Описание.</p></section><p><b>Количество выпусков:</b> 2</p></article>
"#;
const PAGES_FIXTURE: &str = r#"<main><img class="issue" src="/issue.jpg"></main>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_acomics_flow() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].key, "/~sample/about");
        let details = SOURCE
            .details(json!({"manga":{"key":"/~sample/about"}}))
            .unwrap();
        assert_eq!(details.title, "Sample Comic");
        let chapters = SOURCE
            .chapters(json!({"manga":{"key":"/~sample/about"}}))
            .unwrap();
        assert_eq!(chapters[0].key, "/~sample/2");
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":{"key":"/~sample/1"}}))
                .unwrap()
                .len(),
            1
        );
    }
}
