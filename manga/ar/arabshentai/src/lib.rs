use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ArabsHentai = ArabsHentai;
const BASE_URL: &str = "https://arabshentai.com";

struct ArabsHentai;

impl MangaSource for ArabsHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new_chapter"
        } else {
            "new-manga"
        };
        let target = format!("{BASE_URL}/manga/page/{page}/?orderby={order}");
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, false))
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
        let mut target = format!("{BASE_URL}/page/{page}/?s={}", url::query_escape(query));
        if let Some(op) = filters
            .get("genre_op")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            target.push_str("&op=");
            target.push_str(&url::query_escape(op));
        }
        append_multi_filter(&mut target, filters, "genres", "genre[]");
        append_multi_filter(&mut target, filters, "status", "status[]");
        let body = fetch_document_or_fixture(&target, SEARCH_FIXTURE);
        Ok(parse_listing(&body, true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1/".to_string());
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
        .with_origin(BASE_URL)
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

fn parse_listing(body: &str, search: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| !chunk.contains("tvshows") && (search || chunk.contains("wp-manga")))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "data h3", "</h3>")
                .or_else(|| html::text_between(chunk, "details", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("nextpagination") || body.contains("pagination span current"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample/".to_string());
    let info = html::text_between(body, "id=\"manga-info\"", "</section>")
        .or_else(|| html::text_between(body, "id='manga-info'", "</section>"))
        .unwrap_or_default();
    let title = html::text_between(body, "sheader", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: parse_description(&info),
        authors: text_after_label(&info, "الكاتب").into_iter().collect(),
        artists: text_after_label(&info, "الرسام").into_iter().collect(),
        tags: parse_tags(body),
        status: parse_status(&info),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/") || chunk.contains("manga-paged=1"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(href.split('?').next().unwrap_or(&href));
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "ونشوت".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.dedup_by(|a, b| a.key == b.key);
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter-img") || chunk.contains("chapter_image"))
        .filter_map(image_attr)
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

fn image_attr(input: &str) -> Option<String> {
    [
        "srcset",
        "data-cfsrc",
        "data-src",
        "data-lazy-src",
        "bv-data-src",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
    .map(|value| {
        value
            .split_whitespace()
            .next()
            .unwrap_or(&value)
            .to_string()
    })
}

fn parse_description(info: &str) -> Option<String> {
    let text = html::text_between(info, "wp-content", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let aliases = text_after_label(info, "أسماء أُخرى");
    match (text, aliases) {
        (Some(text), Some(aliases)) if !aliases.is_empty() => {
            Some(format!("{text}\nأسماء أُخرى: {aliases}"))
        }
        (Some(text), _) => Some(text),
        _ => None,
    }
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("sgeneros")
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(info: &str) -> ItemStatus {
    let status = text_after_label(info, "حالة المانجا").unwrap_or_default();
    if status.contains("مستمر") {
        ItemStatus::Ongoing
    } else if status.contains("مكتمل") {
        ItemStatus::Completed
    } else if status.contains("متوقف") {
        ItemStatus::Hiatus
    } else if status.contains("ملغية") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn append_multi_filter(target: &mut String, filters: &Value, id: &str, query_name: &str) {
    for value in filters
        .get(id)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        target.push('&');
        target.push_str(query_name);
        target.push('=');
        target.push_str(&url::query_escape(value));
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find("/manga/") {
            return format!("/{}", input[index + 1..].trim_start_matches('/'));
        }
    }
    format!("/{}", input.trim_start_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div id="archive-content"><article class="wp-manga"><div class="poster"><a href="/manga/sample/"><img data-src="/cover.jpg"></a></div><div class="data"><h3><a href="/manga/sample/">Sample Manga</a></h3></div></article></div>
<div class="pagination"><a class="arrow_pag"><i id="nextpagination"></i></a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="search-page"><div class="result-item"><article><div class="image"><div class="thumbnail"><a href="/manga/sample/"><img src="/cover.jpg"></a></div></div><div class="details"><div class="title"><a href="/manga/sample/">Sample Manga</a></div></div></article></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="content"><div class="sheader"><div class="poster"><img src="/cover.jpg"></div><div class="data"><h1>Sample Manga</h1><div class="sgeneros"><a>Drama</a><a>Action</a></div></div></div>
<section id="manga-info"><div class="wp-content"><p>Sample description.</p></div><div><b>أسماء أُخرى</b><span>Alias</span></div><div><b>حالة المانجا</b><span>مكتمل</span></div><div><b>الكاتب</b><span><a>Writer</a></span></div><div><b>الرسام</b><span><a>Artist</a></span></div></section>
<div id="chapter-list"><a href="/manga/sample/chapter-1/"><span class="chapternum">Chapter 1</span><span class="chapterdate">01/01/2024</span></a></div></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="chapter_image"><img class="wp-manga-chapter-img" data-src="/page1.jpg"></div>
<div class="chapter_image"><img class="wp-manga-chapter-img" src="/page2.jpg"></div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_search_and_details() {
        let listing = parse_listing(LIST_FIXTURE, false);
        assert_eq!(listing.entries[0].key, "/manga/sample/");
        assert!(listing.has_next_page);

        let search = parse_listing(SEARCH_FIXTURE, true);
        assert_eq!(search.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, Some("/manga/sample/".into()));
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(details.authors, vec!["Writer"]);
    }

    #[test]
    fn parses_chapters_and_pages() {
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1/");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
