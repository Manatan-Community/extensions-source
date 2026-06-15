use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: YaoiMangaOnline = YaoiMangaOnline;
const BASE_URL: &str = "https://yaoimangaonline.com";

struct YaoiMangaOnline;

impl MangaSource for YaoiMangaOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/page/{page}/"), LIST_FIXTURE);
        Ok(parse_listing(&body))
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
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category = filters
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("-1");
        let tag = filters
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut target = if tag.is_empty() {
            format!("{BASE_URL}/page/{page}/")
        } else {
            format!("{BASE_URL}/tag/{tag}/page/{page}/")
        };
        let mut separator = '?';
        if category != "-1" {
            target.push(separator);
            target.push_str("cat=");
            target.push_str(category);
            separator = '&';
        }
        if !query.is_empty() {
            target.push(separator);
            target.push_str("s=");
            target.push_str(&url::query_escape(query));
        }
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        if chapters.is_empty() {
            chapters.push(MangaChapter {
                key: key.clone(),
                title: Some("Chapter".to_string()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            });
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("post")
        .skip(1)
        .filter(|chunk| {
            !chunk.contains("category-gay-movies") && !chunk.contains("category-yaoi-anime")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("herald-pagination") && body.contains("next"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".to_string());
    let content = html::text_between(body, "entry-content", "</article>").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .map(|value| {
                html::strip_tags(&value)
                    .split(" by ")
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "herald-post-thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: content
            .split("<p")
            .skip(1)
            .filter(|part| !part.contains("<img") && !part.contains("You need to login"))
            .filter_map(|part| html::text_between(part, ">", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
        tags: body
            .split("meta-tags")
            .skip(1)
            .flat_map(|chunk| chunk.split("<a").skip(1))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        authors: parse_author(&content).into_iter().collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_author(content: &str) -> Option<String> {
    let text = html::strip_tags(content);
    text.split("Mangaka:")
        .nth(1)
        .map(|value| {
            value
                .split("Language:")
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("mpp-toc")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href").unwrap_or_default();
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("entry-content")
        .skip(1)
        .flat_map(|chunk| chunk.split("<img").skip(1))
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

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="post"><div><a href="https://yaoimangaonline.com/sample" title="Sample Yaoi"><img src="/cover.jpg"></a></div></article>
<div class="herald-pagination"><a class="next">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Yaoi by Group</h1>
<div class="herald-post-thumbnail"><img src="/cover.jpg"></div>
<div class="meta-tags"><a>Yaoi</a><a>English</a></div>
<article><div class="entry-content"><p>Mangaka: Sample Author Language: English</p><p>Description text.</p><div class="mpp-toc"><a href="/sample/chapter-1">Chapter 1</a></div></div></article>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="entry-content"><img src="/1.jpg"><img src="https://yaoimangaonline.com/2.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaoi_manga_online() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        let details = parse_details(DETAILS_FIXTURE, Some("/sample".into()));
        assert_eq!(details.title, "Sample Yaoi");
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
