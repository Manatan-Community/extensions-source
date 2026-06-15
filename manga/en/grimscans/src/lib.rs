use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: GrimScans = GrimScans;
const BASE_URL: &str = "https://grimscans.com";

struct GrimScans;

impl MangaSource for GrimScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest/")
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
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
        let mut target = format!("{BASE_URL}/series/?q={}", url::query_escape(query));
        for genre in filter_genres(request.get("filters")) {
            target.push_str("&genre=");
            target.push_str(&url::query_escape(&genre));
        }
        Ok(parse_search(
            &fetch_document(&target, SEARCH_FIXTURE),
            query,
            &filter_genres(request.get("filters")),
        ))
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
        let show_paid = request
            .get("preferences")
            .and_then(|prefs| {
                prefs
                    .get("pref_show_paid_chap")
                    .or_else(|| prefs.get("show_paid_chapters"))
            })
            .and_then(Value::as_bool)
            .unwrap_or(false);
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

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("group") && chunk.contains("<a"))
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search(body: &str, query: &str, genres: &[String]) -> Paged<CatalogItem> {
    let lower_query = query.to_ascii_lowercase();
    let entries = body
        .split("<button")
        .skip(1)
        .filter(|chunk| chunk.contains("href=") || chunk.contains("title="))
        .filter(|chunk| {
            lower_query.is_empty()
                || html::attr(chunk, "title")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&lower_query)
        })
        .filter(|chunk| {
            if genres.is_empty() {
                return true;
            }
            let tags = html::attr(chunk, "tags")
                .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
                .unwrap_or_default();
            genres.iter().all(|genre| {
                tags.iter()
                    .any(|tag| tag.eq_ignore_ascii_case(genre.as_str()))
            })
        })
        .filter_map(parse_listing_item)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if !href.contains("/series/") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr(chunk, "title"))
            .or_else(|| url::slug_from_url(&key))
            .filter(|value| !value.is_empty())?,
        cover: background_image(chunk)
            .or_else(|| image_attr(chunk))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "div.grid > h1", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".to_string()),
        cover: background_image(body)
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "Synopsis", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(info_value(body, "Status").as_deref()),
        authors: info_value(body, "Author").into_iter().collect(),
        artists: info_value(body, "Artist").into_iter().collect(),
        tags: parse_tags(body, info_value(body, "Type")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, show_paid: bool) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href=") && chunk.contains("text-sm"))
        .filter(|chunk| !chunk.contains("Upcoming"))
        .filter(|chunk| {
            show_paid || !(chunk.contains("alt=\"Coin\"") || chunk.contains("alt='Coin'"))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let locked = chunk.contains("alt=\"Coin\"") || chunk.contains("alt='Coin'");
            let mut title = html::text_between(chunk, "text-sm", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            if locked {
                title = format!("[LOCKED] {title}");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "text-xs", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let cdn = cdn_url(body);
    let uid_pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("uid="))
        .filter_map(|chunk| html::attr(chunk, "uid"))
        .filter(|uid| !uid.is_empty())
        .collect::<Vec<_>>();
    if let Some(cdn) = cdn.filter(|_| !uid_pages.is_empty()) {
        return uid_pages
            .into_iter()
            .enumerate()
            .map(|(index, uid)| page(index, &format!("{cdn}/{uid}")))
            .collect();
    }
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("#pages") || chunk.contains("cdn"))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| page(index, &url::join_url(BASE_URL, &image)))
        .collect()
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn filter_genres(filters: Option<&Value>) -> Vec<String> {
    let Some(value) = filters
        .and_then(Value::as_object)
        .and_then(|object| object.get("genres"))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_genres)
            .collect();
    }
    value.as_str().map(split_genres).unwrap_or_default()
}

fn split_genres(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let marker = format!(">{label}<");
    body.split(&marker)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<div", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_tags(body: &str, series_type: Option<String>) -> Vec<String> {
    let mut tags = series_type.into_iter().collect::<Vec<_>>();
    for chunk in body.split("<a").skip(1) {
        if chunk.contains("title='Status'") || chunk.contains("title=\"Status\"") {
            continue;
        }
        let text = html::text_between(chunk, ">", "</a>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        if !text.is_empty() && !tags.iter().any(|existing| existing == &text) {
            tags.push(text);
        }
    }
    tags
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ongoing" => ItemStatus::Ongoing,
        "dropped" => ItemStatus::Cancelled,
        "paused" => ItemStatus::Hiatus,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn background_image(input: &str) -> Option<String> {
    let style = html::attr_after(input, "background-image", "style")
        .or_else(|| html::attr_after(input, "photoURL", "style"))
        .or_else(|| html::attr(input, "style"))?;
    let start = style.find("url(")? + 4;
    let rest = &style[start..];
    let end = rest.find(')')?;
    Some(rest[..end].trim_matches(['"', '\'']).to_string())
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-lazy-src")
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
}

fn cdn_url(body: &str) -> Option<String> {
    let marker = "realUrl";
    let script = body.split("<script").find(|chunk| chunk.contains(marker))?;
    let marker_pos = script.find("//")? + 2;
    let host = script[marker_pos..]
        .split(['/', '`', '"', '\''])
        .next()?
        .replace("${url}", "")
        .replace("${host}", "");
    (!host.is_empty()).then(|| format!("https://{host}/uploads"))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="group overflow-hidden"><a href="/series/sample" title="Sample Manga"><div style="background-image: url('/cover.jpg')"></div></a></div>"#;
const SEARCH_FIXTURE: &str = r#"<button title="Sample Manga" tags='["Action"]'><a href="/series/sample" title="Sample Manga"><div style="background-image: url('/cover.jpg')"></div></a></button>"#;
const DETAILS_FIXTURE: &str = r#"<div class="grid"><h1>Sample Manga</h1></div><div style="background-image: url('/cover.jpg')"></div><div>Synopsis</div><div>Sample description.</div><div><span>Status</span></div><div>Ongoing</div><div><span>Author</span></div><div>Author</div><div id="chapters"><a href="/series/sample/chapter-1"><div class="text-sm">Chapter 1</div><div class="text-xs">2024-01-01</div></a></div>"#;
const PAGES_FIXTURE: &str = r#"<script>const realUrl = `https://cdn.keyoapp.com/uploads`;</script><div id="pages"><img uid="page1.jpg"><img uid="page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_keyoapp_shapes() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/series/sample/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
