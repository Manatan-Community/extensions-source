use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LelscanVf = LelscanVf;
const BASE_URL: &str = "https://lelscanfr.com";

struct LelscanVf;

impl MangaSource for LelscanVf {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/?page={page}")
        } else {
            format!("{BASE_URL}/manga?page={page}")
        };
        let body = fetch_document(&target, if latest { LATEST_FIXTURE } else { LIST_FIXTURE });
        Ok(if latest {
            parse_latest(&body)
        } else {
            parse_listing(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut pairs = vec![("title".to_string(), query.trim().to_string())];
    if page > 1 {
        pairs.push(("page".into(), page.to_string()));
    }
    for key in ["type", "status"] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            pairs.push((key.into(), value.to_string()));
        }
    }
    if let Some(genres) = filter_string(filters, "genre") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            pairs.push(("genre[]".into(), genre.to_string()));
        }
    }
    format!(
        "{BASE_URL}/manga?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("id=\"card-real\"")
            .skip(1)
            .filter_map(card_item)
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let section = body
        .split("Chapitres récents")
        .nth(1)
        .or_else(|| body.split("Recent Chapters").nth(1))
        .unwrap_or(body);
    parse_listing(section)
}

fn card_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h2", "</h2>")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Lelscan-VF".into()),
        cover: img_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let mut tags = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("inline-block"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if let Some(kind) = info_value(body, "Type").or_else(|| info_value(body, "النوع")) {
        tags.insert(0, kind);
    }
    CatalogItem {
        key: normalize_key(&key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Lelscan-VF".into()),
        cover: html::attr_after(body, "div class=\"relative", "src")
            .or_else(|| img_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_value(body, "Author")
            .or_else(|| info_value(body, "Auteur"))
            .into_iter()
            .collect(),
        artists: info_value(body, "Artist")
            .or_else(|| info_value(body, "Artiste"))
            .into_iter()
            .collect(),
        description: description(body),
        tags,
        status: info_value(body, "Status")
            .or_else(|| info_value(body, "Statut"))
            .map_or(ItemStatus::Unknown, |value| parse_status(&value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("href=")
                && (chunk.contains("item-title") || chunk.contains("text-gray-500"))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "item-title", "</")
                    .or_else(|| html::text_between(chunk, "<span", "</span>"))
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| body.contains("chapter-container") || chunk.contains("chapter-container"))
        .filter_map(img_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn description(body: &str) -> Option<String> {
    let desc = body
        .split("id=\"description\"")
        .nth(1)
        .or_else(|| body.split("id='description'").nth(1))
        .map(|chunk| html::strip_tags(chunk.split("</div>").next().unwrap_or(chunk)))
        .filter(|value| !value.is_empty());
    let alt = body
        .split("text-sm")
        .nth(1)
        .map(|chunk| html::strip_tags(chunk.split("</span>").next().unwrap_or(chunk)))
        .filter(|value| !value.is_empty());
    match (desc, alt) {
        (Some(desc), Some(alt)) => Some(format!("{desc}\n\nAlternative Title: {alt}")),
        (desc, _) => desc,
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let index = lower.find(&label.to_ascii_lowercase())?;
    let fragment = &body[index..body.len().min(index + 500)];
    html::text_between(fragment, "capitalize", "</")
        .or_else(|| html::text_between(fragment, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "-")
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if ["ongoing", "en cours", "مستمر"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Ongoing
    } else if ["completed", "terminé", "مكتمل"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Completed
    } else if ["dropped", "cancelled", "متوقف"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Cancelled
    } else if value.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("pagination") && !lower.contains("pagination-disabled")
}

fn img_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "srcset")
        .map(|value| {
            value
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn filter_string<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn key_from_input(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        Some(normalize_key(input.trim_start_matches(BASE_URL)))
    } else if input.starts_with("/manga/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(index) = value.find(BASE_URL) {
        return normalize_key(&value[index + BASE_URL.len()..]);
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div id="card-real"><a href="/manga/sample"><img src="/cover.jpg"></a><h2 class="text-sm">Sample</h2></div><ul class="pagination"><li>1</li></ul>
"#;
const LATEST_FIXTURE: &str = r#"<section><h2>Chapitres récents</h2><div id="card-real"><a href="/manga/sample"><img src="/cover.jpg"></a><h2 class="text-sm">Sample</h2></div></section>"#;
const DETAILS_FIXTURE: &str = r#"
<main><section><div><div class="relative"><img src="/cover.jpg"></div><div class="flex"><h1>Sample</h1><div><span class="text-sm">Alt Sample</span></div><a class="inline-block">Action</a></div><div><p id="description">Résumé</p></div></div></section></main>
<div id="buttons"></div><div class="hidden"><p><span>Status</span><span class="capitalize">En cours</span></p><p><span>Author</span><span class="capitalize">Writer</span></p><p><span>Artist</span><span class="capitalize">Artist</span></p></div>
<div id="chapters-list"><a href="/manga/sample/chapter-1"><span id="item-title">Chapitre 1</span><span class="text-gray-500">1 day ago</span></a></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="chapter-container"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
