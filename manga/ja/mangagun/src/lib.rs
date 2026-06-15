use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaGun = MangaGun;
const BASE_URL: &str = "https://nihonkuni.com";

struct MangaGun;

impl MangaSource for MangaGun {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last_update"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &manga_list_url(page, "", "", "", sort, "DESC", ""),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let status = filter_string(&request, "status").unwrap_or("");
        let sort = filter_string(&request, "sort").unwrap_or("last_update");
        let direction = filter_string(&request, "direction").unwrap_or("DESC");
        let genre = filter_string(&request, "genre").unwrap_or("");
        Ok(parse_listing(&fetch_document(
            &manga_list_url(page, query, status, "", sort, direction, genre),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga-sample.html".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/manga-sample.html".to_string());
        let body = fetch_document(&absolute_url(&key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read-sample-chapter-1.html".to_string());
        let target = absolute_url(&key);
        Ok(parse_pages(
            &fetch_document(&target, PAGES_FIXTURE),
            &target,
        ))
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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
        .with_header("Cookie", "smartlink_shown=1")
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

fn manga_list_url(
    page: u64,
    name: &str,
    status: &str,
    author: &str,
    sort: &str,
    direction: &str,
    genre: &str,
) -> String {
    format!(
        "{BASE_URL}/manga-list.html?listType=pagination&page={page}&artist=&author={}&group=&m_status={}&name={}&genre={}&ungenre=&magazine=&sort={}&sort_type={}",
        url::query_escape(author),
        url::query_escape(status),
        url::query_escape(name),
        url::query_escape(genre),
        url::query_escape(sort),
        url::query_escape(direction),
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "manga-title", "</")
                .or_else(|| html::text_between(chunk, "<h", "</h"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "NihonKuni".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-link next") && body.contains("href"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    let info = body.split("manga-detail-container").nth(1).unwrap_or(&body);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(info, "<h1", "</h1>")
            .or_else(|| html::text_between(info, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "NihonKuni".into())),
        cover: image_attr(info).map(|image| absolute_url(&image)),
        description: html::text_between(&body, "description-text-content", "</")
            .or_else(|| html::text_between(&body, "manga-info-list", "</ul>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: link_texts(info, "author"),
        tags: link_texts(info, "genre"),
        status: parse_status(&html::strip_tags(info)),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-name") || chunk.contains("at-series"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-name", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapter-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page"))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-original")
        .or_else(|| html::attr_after(input, "<img", "data-src"))
        .or_else(|| html::attr_after(input, "<img", "data-bg"))
        .or_else(|| html::attr_after(input, "<img", "data-srcset"))
        .or_else(|| style_url(input))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn style_url(input: &str) -> Option<String> {
    input
        .split("url(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|value| value.trim_matches(['\'', '"', ' ']).to_string())
        .filter(|value| !value.is_empty())
}

fn link_texts(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("Updating"))
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("completed") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("ongoing")
        || lower.contains("updating")
        || lower.contains("incomplete")
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

const LIST_FIXTURE: &str = r#"
<div class="manga-card"><a class="manga-title" href="/manga-sample.html">Sample Manga</a><img class="manga-cover" src="/cover.jpg"></div>
<a class="page-link next" href="/manga-list.html?page=2">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="manga-detail-container"><h1>Sample Manga</h1><img class="manga-cover" src="/cover.jpg"><ul><li><a href="/author/writer">Writer</a></li><li><a href="/genre/Action">Action</a></li><li>Ongoing</li></ul></div>
<div class="description-text-content">Sample description.</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div class="at-series"><a href="/read-sample-chapter-1.html"><span class="chapter-name">Chapter 1</span><span class="chapter-time">2024-01-01</span></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<img id="page1" src="/page1.jpg"><img id="page2" src="/page2.jpg">
"#;

export_manga_source!(SOURCE);
