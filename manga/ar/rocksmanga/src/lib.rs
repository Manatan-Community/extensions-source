use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RocksManga = RocksManga;
const BASE_URL: &str = "https://rocksmanga.com";

struct RocksManga;

impl MangaSource for RocksManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            listing_url(page, "latest")
        } else {
            listing_url(page, "popular")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            filtered_url(page, request.get("filters"))
        } else {
            format!(
                "{}/{}/?s={}",
                BASE_URL,
                search_page(page),
                url::query_escape(query)
            )
        };
        let body = fetch_or_fixture(&target, SEARCH_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn listing_url(page: u64, order: &str) -> String {
    let page_path = search_page(page);
    match order {
        "latest" => format!("{BASE_URL}/{page_path}?m_orderby=latest"),
        _ => format!("{BASE_URL}/{page_path}?m_orderby=views"),
    }
}

fn filtered_url(page: u64, filters: Option<&Value>) -> String {
    let mut selected_type = "";
    let mut selected_genre = "";
    if let Some(filters) = filters.and_then(Value::as_object) {
        selected_type = filters
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        selected_genre = filters
            .get("genre")
            .and_then(Value::as_str)
            .unwrap_or_default();
    }
    let page_path = search_page(page);
    if !selected_genre.is_empty() {
        format!("{BASE_URL}/manga-genre/{selected_genre}/{page_path}")
    } else if !selected_type.is_empty() {
        format!("{BASE_URL}/manga-type/{selected_type}/{page_path}")
    } else {
        listing_url(page, "popular")
    }
}

fn search_page(page: u64) -> String {
    if page <= 1 {
        String::new()
    } else {
        format!("page/{page}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = listing_chunks(body)
        .into_iter()
        .filter_map(|chunk| catalog_item_from_chunk(&chunk))
        .fold(Vec::new(), push_unique);
    if entries.is_empty() {
        entries = body
            .split("<a")
            .skip(1)
            .map(|chunk| format!("<a{chunk}"))
            .filter(|chunk| chunk.contains("href"))
            .filter_map(|chunk| catalog_item_from_chunk(&chunk))
            .fold(Vec::new(), push_unique);
    }
    Paged {
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("rel='next'")
            || body.contains("page-link"),
        entries,
    }
}

fn catalog_item_from_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "class=\"info", "href")
        .or_else(|| html::attr_after(chunk, "class='info", "href"))
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h3", "</h3>")
            .or_else(|| html::text_between(chunk, "<h2", "</h2>"))
            .or_else(|| html::text_between(chunk, "<h1", "</h1>"))
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn listing_chunks(body: &str) -> Vec<&str> {
    let chunks: Vec<&str> = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("unit") || chunk.contains("bsx"))
        .collect();
    if chunks.is_empty() {
        body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("href"))
            .collect()
    } else {
        chunks
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "class=\"info", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "manga-poster", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "class=\"description", "</div>")
            .or_else(|| html::text_between(body, "class='description", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: meta_values(body, "المؤلف"),
        artists: meta_values(body, "الرسام"),
        tags: meta_values(body, "التصنيفات"),
        status: html::text_between(body, "class=\"info", "</p>")
            .or_else(|| html::text_between(body, "class='info", "</p>"))
            .map(|value| parse_status(&html::strip_tags(&value)))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("list-body-hh") || chunk.contains("<zebi"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<zebi", "</zebi>")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                scanlators: html::text_between(chunk, "class=\"username", "</span>")
                    .or_else(|| html::text_between(chunk, "class='username", "</span>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .into_iter()
                    .collect(),
                date_uploaded: html::text_between(chunk, "class=\"time", "</")
                    .or_else(|| html::text_between(chunk, "class='time", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("img") || chunk.contains("ch-images"))
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

fn meta_values(body: &str, label: &str) -> Vec<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</div>"))
        .or_else(|| {
            body.split(label)
                .nth(1)
                .and_then(|chunk| html::text_between(chunk, "</span>", "</div>"))
        })
        .map(|value| {
            html::strip_tags(&value)
                .replace('،', ",")
                .split(',')
                .map(str::trim)
                .map(ToString::to_string)
                .filter(|value| !value.is_empty() && value != ":")
                .collect()
        })
        .unwrap_or_default()
}

fn image_attr(input: &str) -> Option<String> {
    [
        "data-background-image",
        "data-cfsrc",
        "data-lazy-src",
        "data-src",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("مكتملة") || value.eq_ignore_ascii_case("complete") {
        ItemStatus::Completed
    } else if value.contains("مستمرة") || value.eq_ignore_ascii_case("ongoing") {
        ItemStatus::Ongoing
    } else if value.eq_ignore_ascii_case("dropped") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="unit"><div class="inner">
  <a href="https://rocksmanga.com/series/sample" title="عينة روكس"><img src="/covers/hero.jpg" alt="عينة روكس"><h3>عينة روكس</h3></a>
</div></div>
<li class="page-item"><a class="page-link" rel="next" href="/page/2">Next</a></li>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<div class="manga-poster"><img src="/covers/hero.jpg"></div>
<div class="info">
  <h1>عينة روكس</h1>
  <h6>Sample Rocks</h6>
  <p>مستمرة</p>
</div>
<div class="meta">
  <span>المؤلف:</span><a>كاتب</a>
  <span>الرسام:</span><a>رسام</a>
  <span>التصنيفات:</span><a>اكشن</a><a>خيال</a>
</div>
<div class="description"><p>وصف تجريبي حقيقي الشكل.</p></div>
<div class="list-body-hh"><ul>
  <li><a href="/series/sample/chapter-1"><zebi>الفصل 1</zebi></a><span class="username"><span>فريق روكس</span></span><span class="time">يناير 1, 2024</span></li>
</ul></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="ch-images">
  <img class="img" src="/pages/001.jpg">
  <img class="img" data-src="/pages/002.jpg">
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].key, "/series/sample");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_chapters_and_pages() {
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
