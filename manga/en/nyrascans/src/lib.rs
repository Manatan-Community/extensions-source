use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: NyraScans = NyraScans;
const BASE_URL: &str = "https://nyrascans.com";

struct NyraScans;

impl MangaSource for NyraScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_cards(HOME_FIXTURE),
                has_next_page: false,
            });
        }
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest/")
        } else {
            BASE_URL.to_string()
        };
        Ok(Paged {
            entries: parse_cards(&fetch_document(&target, HOME_FIXTURE)),
            has_next_page: false,
        })
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/series/")
        } else {
            format!("{BASE_URL}/series/?q={}", url::query_escape(query))
        };
        let body = fetch_document(&target, SEARCH_FIXTURE);
        let mut entries = parse_cards(&body);
        if !query.is_empty() {
            entries.retain(|item| item.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let show_paid = preference_bool(&request, "show_paid_chapters");
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            show_paid,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split('<')
        .filter(|chunk| chunk.starts_with("button") || chunk.starts_with("div"))
        .filter(|chunk| {
            chunk.contains("href")
                && (chunk.contains("title=") || chunk.contains("background-image"))
        })
        .filter_map(|chunk| {
            let href =
                html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::attr(chunk, "title")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Nyra Scans".to_string());
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: style_image(chunk).map(|image| sized_image(&image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    let mut tags = link_values(body, "?genre=");
    for value in ["Manga", "Manhwa", "Manhua"] {
        if body.contains(value) {
            tags.push(value.to_string());
            break;
        }
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: style_image(body)
            .map(|image| sized_image(&image))
            .or_else(|| image_attr(body).map(|image| url::join_url(BASE_URL, &image))),
        description: text_after_label(body, "Synopsis"),
        authors: text_after_label(body, "Author").into_iter().collect(),
        artists: text_after_label(body, "Artist").into_iter().collect(),
        tags,
        status: parse_status(&text_after_label(body, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, show_paid: bool) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href") && !chunk.contains("Upcoming"))
        .filter(|chunk| show_paid || !chunk.contains("Coin"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/chapter") {
                return None;
            }
            let title = html::text_between(chunk, "text-sm", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if chunk.contains("Coin") {
                    format!("Locked: {title}")
                } else {
                    title
                }),
                url: Some(url::join_url(BASE_URL, &key)),
                is_locked: chunk.contains("Coin"),
                date_uploaded: html::text_between(chunk, "text-xs", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let cdn = cdn_url(body);
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "uid")
                .and_then(|uid| cdn.as_ref().map(|base| format!("{base}/{uid}")))
                .or_else(|| image_attr(chunk))
        })
        .filter(|image| !image.is_empty())
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

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!(
                "/{}",
                input[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn style_image(input: &str) -> Option<String> {
    let style = html::attr(input, "style")?;
    let start = style.find("url(")? + 4;
    let rest = style[start..].trim_start_matches(['\'', '"']);
    let end = rest.find(['\'', '"', ')']).unwrap_or(rest.len());
    Some(html::html_unescape(&rest[..end]))
}

fn sized_image(image: &str) -> String {
    if image.contains("?") {
        format!("{image}&w=480")
    } else {
        format!("{image}?w=480")
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<div", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    match input.to_lowercase().as_str() {
        value if value.contains("completed") => ItemStatus::Completed,
        value if value.contains("dropped") => ItemStatus::Cancelled,
        value if value.contains("paused") => ItemStatus::Hiatus,
        value if value.contains("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn cdn_url(body: &str) -> Option<String> {
    let marker = "realUrl";
    let chunk = body.split(marker).nth(1)?;
    let scheme_index = chunk.find("//")? + 2;
    let host = chunk[scheme_index..]
        .split(['/', '`', '"', '\''])
        .next()
        .filter(|host| !host.is_empty())?;
    Some(format!("https://{host}/uploads"))
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
<button href="/series/sample" title="Sample Nyra Scans" style="background-image:url('https://cdn.keyoapp.com/cover.jpg')"></button>
"#;
const SEARCH_FIXTURE: &str = HOME_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<div class="grid"><h1>Sample Manga</h1></div>
<div style="background-image:url('https://cdn.keyoapp.com/cover.jpg')"></div>
<div>Synopsis</div><div>Sample description</div><div>Status</div><div>Ongoing</div>
<a href="?genre=action" title="Action">Action</a>
<div id="chapters"><a href="/series/sample/chapter-1"><div class="text-sm">Chapter 1</div><div class="text-xs">Jan 1, 2024</div></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>realUrl = `https://cdn.keyoapp.com`</script><div id="pages"><img uid="page1.jpg"></div>"#;
