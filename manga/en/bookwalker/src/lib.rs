use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BookWalker = BookWalker;
const BASE_URL: &str = "https://global.bookwalker.jp";
const MEMBER_API_URL: &str = "https://member-app.bookwalker.jp/api";
const RIMG_URL: &str = "https://rimg.bookwalker.jp";
const C_URL: &str = "https://c.bookwalker.jp";

struct BookWalker;

impl MangaSource for BookWalker {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE),
                has_next_page: has_next_page(LIST_FIXTURE),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/new/?order=release&qcat=2&np=0&page={page}")
        } else {
            format!("{BASE_URL}/categories/2/?order=rank&np=0&page={page}")
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![details_from_key(&normalize_key(query))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/search/?np=0&page={page}&word={}&order={}",
            url::query_escape(query),
            if query.is_empty() { "rank" } else { "score" }
        );
        let body = fetch_document(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1/".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/1/".to_string());
        if !key.starts_with("/series/") {
            let body = fetch_document(&url::join_url(BASE_URL, &key), CHAPTER_PAGE_FIXTURE);
            return Ok(chapter_from_page(&body, &key).into_iter().collect());
        }

        let first_url = format!("{}?order=release&page=1", url::join_url(BASE_URL, &key));
        let first = fetch_document(&first_url, LIST_FIXTURE);
        let mut chapters = parse_chapter_tiles(&first);
        for page in 2..=page_count(&first) {
            let body = fetch_document(
                &format!(
                    "{}?order=release&page={page}",
                    url::join_url(BASE_URL, &key)
                ),
                LIST_FIXTURE,
            );
            chapters.extend(parse_chapter_tiles(&body));
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/de000000000000/".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), CHAPTER_PAGE_FIXTURE);
        let page_count = html::text_between(&body, "Page count", "</td>")
            .and_then(|value| first_number(&html::strip_tags(&value)))
            .unwrap_or(0);
        let message = if page_count > 0 {
            format!(
                "BookWalker reports {page_count} pages, but this Manatan WASM port cannot read them. The upstream reader requires Android WebView canvas capture plus Publus authenticated image interception."
            )
        } else {
            "BookWalker pages are unsupported in this Manatan WASM port. The upstream reader requires Android WebView canvas capture plus Publus authenticated image interception.".to_string()
        };
        Ok(vec![manga::text_page(&message)])
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
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&normalize_key(input))),
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
        .with_cookies_for(MEMBER_API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("o-tile")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "a-tile-ttl", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "a-tile-ttl", "title")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| html::text_between(chunk, "a-tile-ttl", "</"))
                .map(|value| clean_title(&html::strip_tags(&value)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Book".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: tile_cover(chunk),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
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

fn details_from_key(key: &str) -> CatalogItem {
    if key.starts_with("/series/") {
        return series_details(key);
    }
    single_details(key)
}

fn series_details(key: &str) -> CatalogItem {
    let body = fetch_document(
        &format!("{}?order=release&np=1", url::join_url(BASE_URL, key)),
        LIST_FIXTURE,
    );
    let chapters = parse_chapter_tiles(&body);
    let latest = chapters
        .first()
        .and_then(|chapter| uuid_from_key(&chapter.key));
    let earliest = chapters
        .last()
        .and_then(|chapter| uuid_from_key(&chapter.key));
    let update = earliest
        .as_deref()
        .and_then(fetch_book_update)
        .or_else(|| latest.as_deref().and_then(fetch_book_update))
        .unwrap_or_default();
    CatalogItem {
        key: normalize_key(key),
        title: clean_title(
            update
                .series_name
                .as_deref()
                .unwrap_or(update.product_name.as_str()),
        ),
        authors: available_filter_names(&body, "side-author"),
        description: Some(
            [
                update.product_explanation_short.unwrap_or_default(),
                update.product_explanation_details,
            ]
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        )
        .filter(|value| !value.is_empty()),
        cover: latest
            .as_deref()
            .and_then(fetch_book_update)
            .and_then(|item| item.cover_image_url)
            .or(update.cover_image_url),
        tags: available_filter_names(&body, "side-genre"),
        status: if body.contains("Completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn single_details(key: &str) -> CatalogItem {
    let update = uuid_from_key(key)
        .as_deref()
        .and_then(fetch_book_update)
        .unwrap_or_default();
    CatalogItem {
        key: normalize_key(key),
        title: clean_title(
            update
                .series_name
                .as_deref()
                .unwrap_or(update.product_name.as_str()),
        ),
        authors: update
            .authors
            .into_iter()
            .map(|author| author.author_name)
            .collect(),
        description: Some(
            [
                update.product_explanation_short.unwrap_or_default(),
                update.product_explanation_details,
            ]
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        )
        .filter(|value| !value.is_empty()),
        cover: update.cover_image_url,
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_tiles(body: &str) -> Vec<MangaChapter> {
    body.split("o-tile")
        .skip(1)
        .filter(|chunk| !chunk.contains("a-ribbon-bundle"))
        .filter_map(chapter_from_tile)
        .collect()
}

fn chapter_from_tile(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr_after(chunk, "a-tile-ttl", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let title = html::attr_after(chunk, "a-tile-ttl", "title")
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .or_else(|| html::text_between(chunk, "a-tile-ttl", "</"))
        .map(|value| clean_title(&html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Chapter".to_string());
    let status = if chunk.contains("a-label-free") {
        " [free]"
    } else if chunk.contains("a-cart-btn") {
        " [purchase]"
    } else if chunk.contains("a-order-btn") {
        " [preorder]"
    } else {
        ""
    };
    let key = normalize_key(&href);
    let chapter_number = parse_chapter_number(&title);
    Some(MangaChapter {
        key: key.clone(),
        title: Some(format!(
            "{}{}",
            chapter_number
                .map(|number| {
                    if number.fract() == 0.0 {
                        format!("Chapter {}", number as u64)
                    } else {
                        format!("Chapter {number}")
                    }
                })
                .unwrap_or(title),
            status
        )),
        chapter_number,
        url: Some(url::join_url(BASE_URL, &key)),
        ..MangaChapter::default()
    })
}

fn chapter_from_page(body: &str, key: &str) -> Option<MangaChapter> {
    let title = html::text_between(body, "detail-book-title-box", "</h1>")
        .map(|value| clean_title(&html::strip_tags(&value)))
        .filter(|value| !value.is_empty())?;
    Some(MangaChapter {
        key: normalize_key(key),
        title: Some(title.clone()),
        chapter_number: parse_chapter_number(&title),
        scanlators: html::text_between(body, "Publisher", "</td>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        url: Some(url::join_url(BASE_URL, key)),
        ..MangaChapter::default()
    })
}

fn fetch_book_update(uuid: &str) -> Option<BookUpdate> {
    let target = format!(
        "{MEMBER_API_URL}/books/updates?fileType=EPUB&{}=0",
        url::query_escape(uuid)
    );
    let body = fetch_api(&target, BOOK_UPDATE_FIXTURE);
    serde_json::from_str::<Vec<BookUpdate>>(&body)
        .ok()
        .and_then(|mut values| values.pop())
}

fn has_next_page(body: &str) -> bool {
    body.contains("pager-area") && body.contains("next")
}

fn page_count(body: &str) -> u64 {
    html::text_between(body, "pager-area", "</ul>")
        .and_then(|pager| {
            pager
                .split("<a")
                .filter_map(|chunk| html::strip_tags(chunk).parse::<u64>().ok())
                .max()
        })
        .unwrap_or(1)
}

fn available_filter_names(body: &str, class_name: &str) -> Vec<String> {
    body.split(&format!("ul class=\"{class_name}"))
        .nth(1)
        .unwrap_or_default()
        .split("<span")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn tile_cover(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-srcset")
        .and_then(|value| highest_srcset(&value))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .and_then(|value| hi_res_cover(&value))
}

fn highest_srcset(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .filter_map(|entry| {
            let mut parts = entry.split_whitespace();
            let url = parts.next()?.to_string();
            let scale = parts
                .next()
                .and_then(|value| value.trim_end_matches('x').parse::<u64>().ok())
                .unwrap_or(1);
            Some((scale, url))
        })
        .max_by_key(|(scale, _)| *scale)
        .map(|(_, value)| value)
}

fn hi_res_cover(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let extension = value.rsplit('.').next().unwrap_or("jpg");
    if value.starts_with(RIMG_URL) {
        let id = value
            .trim_start_matches(RIMG_URL)
            .trim_start_matches('/')
            .split('/')
            .next()
            .and_then(|id| id.chars().rev().collect::<String>().parse::<i64>().ok());
        return id.map(|id| format!("{C_URL}/coverImage_{}.{}", id - 1, extension));
    }
    Some(value.to_string())
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        format!("/{}", value[BASE_URL.len()..].trim_matches('/'))
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn uuid_from_key(key: &str) -> Option<String> {
    key.split("/de")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn first_number(value: &str) -> Option<u64> {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn parse_chapter_number(value: &str) -> Option<f32> {
    let lower = value.to_ascii_lowercase();
    for marker in ["vol.", "volume", "chapter", "#"] {
        if let Some(rest) = lower.split(marker).nth(1) {
            let number = rest
                .trim()
                .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
                .next()
                .unwrap_or_default();
            if let Ok(value) = number.parse::<f32>() {
                return Some(value);
            }
        }
    }
    None
}

fn clean_title(value: &str) -> String {
    [
        "(manga)",
        "(comic)",
        "<serial>",
        "(serial)",
        "<chapter release>",
    ]
    .into_iter()
    .fold(value.to_string(), |title, marker| {
        title
            .replace(marker, "")
            .replace(&marker.to_ascii_uppercase(), "")
    })
    .trim()
    .to_string()
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookUpdate {
    product_name: String,
    series_name: Option<String>,
    product_explanation_short: Option<String>,
    #[serde(default)]
    product_explanation_details: String,
    cover_image_url: Option<String>,
    #[serde(default)]
    authors: Vec<AuthorUpdate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorUpdate {
    author_name: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="book-list-area"><div class="o-tile"><div class="a-tile-ttl"><a href="https://global.bookwalker.jp/series/1/" title="Sample Manga (Manga)">Sample Manga</a></div><div class="a-tile-thumb-img"><img data-srcset="https://rimg.bookwalker.jp/123/cover.jpg 1x, https://rimg.bookwalker.jp/321/cover.jpg 2x"></div></div></div>
<div class="pager-area"><span class="next"><a href="?page=2">Next</a></span></div>
"#;
const CHAPTER_PAGE_FIXTURE: &str = r#"
<div class="detail-book-title-box"><h1 itemprop="name">Sample Chapter 1</h1></div>
<table class="product-detail"><tr><th>Page count</th><td>12 pages</td></tr><tr><th>Publisher</th><td>Sample Publisher</td></tr></table>
"#;
const BOOK_UPDATE_FIXTURE: &str = r#"
[{"productName":"Sample Manga Chapter 1","seriesName":"Sample Manga","productExplanationShort":"Short description","productExplanationDetails":"Long description","coverImageUrl":"https://c.bookwalker.jp/coverImage_1.jpg","authors":[{"authorName":"Sample Author"}]}]
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_listing_fixture() {
        let page = SOURCE.list(json!({})).unwrap();
        assert_eq!(page.entries[0].title, "Sample Manga");
    }
}
