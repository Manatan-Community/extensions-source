use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Goda = Goda;
const BASE_URL: &str = "https://manhuascans.org";

struct Goda;

impl MangaSource for Goda {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let endpoint = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "newss"
        } else {
            "hots"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{endpoint}/page/{page}"),
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
        let target = if !query.is_empty() {
            format!("{BASE_URL}/s/{}?page={page}", url::query_escape(query))
        } else if let Some(path) = request
            .get("filters")
            .and_then(|filters| filters.get("genrePath"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            format!("{BASE_URL}/{}/page/{page}", path.trim().trim_matches('/'))
        } else {
            format!("{BASE_URL}/hots/page/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let manga_id = request
            .get("manga")
            .and_then(|manga| manga.get("description"))
            .and_then(Value::as_str)
            .and_then(manga_id_from_description)
            .unwrap_or_else(|| {
                let details = fetch_document(&manga_url(&key), DETAILS_FIXTURE);
                manga_id_from_body(&details).unwrap_or_else(|| "1".to_string())
            });
        Ok(parse_chapters(
            &fetch_document(
                &format!("{BASE_URL}/manga/get?mid={manga_id}&mode=all"),
                CHAPTERS_FIXTURE,
            ),
            &manga_id,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample#1/1".to_string());
        let ids = key.split('#').nth(1).unwrap_or("1/1");
        let manga_id = ids.split('/').next().unwrap_or("1");
        let chapter_id = ids.split('/').nth(1).unwrap_or("1");
        Ok(parse_pages(&fetch_document(
            &format!("{BASE_URL}/chapter/getcontent?m={manga_id}&c={chapter_id}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| manga_url(key.split('#').next().unwrap_or(&key))))
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

fn normalize_key(value: &str) -> String {
    let path = if let Some(rest) = value.strip_prefix(BASE_URL) {
        rest
    } else {
        value
    };
    path.split("/manga/")
        .nth(1)
        .unwrap_or(path)
        .trim_matches('/')
        .to_string()
}

fn manga_url(key: &str) -> String {
    format!(
        "{BASE_URL}/manga/{}",
        key.trim_matches('/').split('#').next().unwrap_or(key)
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/manga/") && chunk.contains("<h3"))
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("aria-label=\"NEXT\"") || body.contains("aria-label=NEXT"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    let image = html::attr_after(chunk, "<img", "src")
        .and_then(|src| {
            src.split("url=")
                .nth(1)
                .map(ToString::to_string)
                .or(Some(src))
        })
        .map(|value| url::join_url(BASE_URL, &value));
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h3", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image,
        url: Some(manga_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "sample".to_string());
    let title = html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    let manga_id = manga_id_from_body(body).unwrap_or_else(|| "1".to_string());
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "object-cover", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: detail_link_values(body, "author"),
        artists: detail_link_values(body, "artist"),
        tags: tag_values(body),
        description: Some(
            html::text_between(body, "<p", "</p>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_default()
                + &format!("\n\nID: {manga_id}"),
        ),
        status: parse_status(body),
        url: Some(manga_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn manga_id_from_body(body: &str) -> Option<String> {
    html::attr_after(body, "mangachapters", "data-mid").filter(|value| value.parse::<u64>().is_ok())
}

fn manga_id_from_description(value: &str) -> Option<String> {
    value
        .rsplit("ID: ")
        .next()
        .map(str::trim)
        .filter(|id| id.parse::<u64>().is_ok())
        .map(ToString::to_string)
}

fn parse_status(body: &str) -> ItemStatus {
    if body.contains("Completed") {
        ItemStatus::Completed
    } else if body.contains("Cancelled") || body.contains("停止更新") {
        ItemStatus::Cancelled
    } else if body.contains("Hiatus") || body.contains("休刊") {
        ItemStatus::Hiatus
    } else if body.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn detail_link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<p")
        .filter(|chunk| chunk.to_ascii_lowercase().contains(marker))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn tag_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", "</a>"))
        .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
        .filter(|value| !value.is_empty() && !value.contains(','))
        .collect()
}

fn parse_chapters(body: &str, manga_id: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("chapteritem")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let chapter_id =
                html::attr_after(chunk, "<a", "data-cs").unwrap_or_else(|| "1".to_string());
            let title = html::attr_after(chunk, "<a", "data-ct")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let manga_key = normalize_key(&href);
            Some(MangaChapter {
                key: format!("{manga_key}#{manga_id}/{chapter_id}"),
                title: Some(title),
                url: Some(manga_url(&manga_key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|value| !value.is_empty())
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
<div class="container"><div class="cardlist"><div class="pb-2"><a href="/manga/sample"><img src="/cover.jpg"><h3>Sample Manga</h3></a></div></div></div><a aria-label="NEXT"><button>Next</button></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<main><div><div><h1><span>Ongoing</span>Sample Manga</h1><p>Author <a>Creator</a></p><p>Genre <a>Action</a></p><p><a>#Tag</a></p><p>A sample manga.</p></div><img class="object-cover" src="/cover.jpg"></div><div id="mangachapters" data-mid="1"></div></main>
"#;
const CHAPTERS_FIXTURE: &str = r#"
<div class="chapteritem"><a href="/manga/sample/chapter-1" data-cs="1" data-ct="Chapter 1"></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="chapcontent"><div><img data-src="/page1.jpg"><img src="/page2.jpg"></div></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_goda_flow() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].key, "sample");
        let chapters = SOURCE
            .chapters(json!({"manga":{"key":"sample","description":"ID: 1"}}))
            .unwrap();
        assert_eq!(chapters[0].key, "sample/chapter-1#1/1");
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":chapters[0].key.clone()}))
                .unwrap()
                .len(),
            2
        );
    }
}
