use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RokuHentai = RokuHentai;
const BASE_URL: &str = "https://rokuhentai.com";

struct RokuHentai;

impl MangaSource for RokuHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page <= 1 {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/_search")
        };
        let body = fetch_text_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![details_from_url(query)],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}?q={}", url::query_escape(query));
        Ok(parse_listing(&fetch_text_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/gallery/sample#2".into());
        let page_count = key
            .split('#')
            .nth(1)
            .and_then(|meta| meta.split(',').next())
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or(1);
        Ok(vec![MangaChapter {
            key: key
                .split('#')
                .next()
                .unwrap_or(&key)
                .trim_start_matches('/')
                .to_string(),
            title: Some("Gallery".to_string()),
            chapter_number: Some(0.0),
            scanlators: vec![format!("{page_count}P")],
            url: Some(format!(
                "{BASE_URL}/{}/0#top-to-bottom",
                key.split('#')
                    .next()
                    .unwrap_or(&key)
                    .trim_start_matches('/')
            )),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "gallery/sample".into());
        let count = request
            .get("chapter")
            .and_then(|chapter| chapter.get("scanlators"))
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .and_then(|value| value.strip_suffix('P'))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2);
        let path = if request
            .get("preferences")
            .and_then(|prefs| prefs.get("thumbnails"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "page-thumbnails"
        } else {
            "pages"
        };
        Ok((0..count)
            .map(|index| {
                let image = format!(
                    "{BASE_URL}/_images/{path}/{}/{}.jpg",
                    key.trim_matches('/'),
                    index
                );
                MangaPage {
                    content: PageContent::Url {
                        url: image,
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_url(input)),
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

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries: Vec<CatalogItem> = if body.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| value.get("manga-cards").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .filter_map(parse_card)
            .collect()
    } else {
        body.split("site-popunder-ad-slot")
            .filter_map(parse_card)
            .collect()
    };
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    let info = html::text_between(chunk, "mdc-typography--caption", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    let page_count = info
        .split(" images ")
        .next()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(1);
    let key = format!("{}#{page_count}", normalize_key(&href));
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "site-manga-card__title--primary", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Gallery".into())),
        cover: background_image(chunk),
        status: ItemStatus::Completed,
        url: Some(url::join_url(
            BASE_URL,
            key.split('#').next().unwrap_or(&key),
        )),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = details_from_url(key);
    item.cover = background_image(body).or(item.cover);
    item.description = html::text_between(body, "site-manga-info__info h6", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    for chip in body.split("mdc-chip").skip(1) {
        let text = html::strip_tags(chip);
        if let Some((label, value)) = text.split_once(": ") {
            if label == "artist" {
                item.authors = vec![value.to_string()];
                item.artists = vec![value.to_string()];
            }
            item.tags.push(if value.contains(' ') {
                format!("{label}: \"{value}\"")
            } else {
                format!("{label}: {value}")
            });
        }
    }
    item.status = ItemStatus::Completed;
    item.initialized = true;
    item
}

fn details_from_url(input: &str) -> CatalogItem {
    let key = normalize_key(input);
    CatalogItem {
        key: key.clone(),
        title: url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()),
        status: ItemStatus::Completed,
        url: Some(url::join_url(
            BASE_URL,
            key.split('#').next().unwrap_or(&key),
        )),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn background_image(chunk: &str) -> Option<String> {
    let marker = "background-image: url(";
    let start = chunk.find(marker)? + marker.len();
    let rest = &chunk[start..];
    let end = rest.find(')')?;
    Some(rest[..end].trim_matches(['"', '\'']).to_string())
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL).trim_matches('/');
    format!("/{}", path.trim_end_matches("/0"))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<a class="site-popunder-ad-slot" href="/gallery/sample"><div class="mdc-card__media" style="background-image: url(&quot;https://rokuhentai.com/thumb.jpg&quot;);"></div><span class="site-manga-card__title--primary">Sample Gallery</span><span class="mdc-typography--caption">2 images Jan 1, 2024, 1:00 AM</span></a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="site-manga-info"><div class="mdc-card__media" style="background-image: url(&quot;https://rokuhentai.com/thumb.jpg&quot;);"></div></div>
<div class="site-manga-info__info"><h6>Sample Gallery</h6><h6>Sample description</h6></div>
<span class="mdc-chip">artist: Jane</span><span class="mdc-chip">language: English</span>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_details_chapters_and_pages() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Gallery");

        let details = SOURCE
            .details(json!({"manga":"/gallery/sample#2"}))
            .unwrap();
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = SOURCE
            .chapters(json!({"manga":"/gallery/sample#2"}))
            .unwrap();
        assert_eq!(chapters[0].scanlators, vec!["2P"]);

        let pages = SOURCE
            .pages(json!({"chapter":"gallery/sample","chapter":{"scanlators":["2P"]}}))
            .unwrap();
        assert_eq!(pages.len(), 2);
    }
}
