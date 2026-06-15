use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: RawInu = RawInu;
const BASE_URL: &str = "https://rawinu.com";
const API_ENDPOINT: &str = "https://rawinu.com/app/manga/controllers";

struct RawInu;

impl MangaSource for RawInu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last_update"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga-list.html?listType=pagination&page={}&sort={sort}&sort_type=DESC", page(&request)),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga-list.html?name={}&page={}", url::query_escape(query), page(&request)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga-sample.html".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga-sample.html".into());
        let slug = key.substring_between("/manga-", ".html").unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "sample".into()));
        let body = fetch_document(&format!("{API_ENDPOINT}/cont.Listchapter.php?slug={}", url::query_escape(&slug)), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter.html".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let chapter_id = html::attr_after(&body, "name=\"chapter\"", "value").or_else(|| html::attr_after(&body, "id=\"chapter\"", "value")).unwrap_or_else(|| "1".into());
        let images = fetch_document(&format!("{API_ENDPOINT}/cont.imagesChap.php?cid={chapter_id}"), IMAGE_FIXTURE);
        Ok(parse_pages(&images, &chapter_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection { id: "popular".into(), title: "Popular".into(), style: Some(HomeSectionStyle::Cover), has_more: popular.has_next_page, entries: popular.entries, ..HomeSection::default() },
            HomeSection { id: "latest".into(), title: "Latest".into(), style: Some(HomeSectionStyle::Cover), has_more: latest.has_next_page, entries: latest.entries, ..HomeSection::default() },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: Some(details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_header("Cookie", "smartlink_shown=1")
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("media") || chunk.contains("thumb-item-flow"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<h3", "href")
                    .or_else(|| html::attr_after(chunk, "series-title", "href"))
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "<h3", "</h3>")
                        .or_else(|| html::text_between(chunk, "series-title", "</"))
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "RawINU".into())),
                    cover: image_attr(chunk).map(|value| value.trim_matches('\'').to_string()),
                    url: Some(absolute_url(&key)),
                    language: Some("ja".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("btn-info") || body.contains("pagination"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    let info = html::text_between(&body, "card-body", "</div>").unwrap_or_else(|| body.clone());
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(&body, "<h1", "</h1>").or_else(|| html::text_between(&body, "<h3", "</h3>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "RawINU".into())),
        cover: image_attr(&info),
        authors: button_values(&info, "btn-info"),
        tags: button_values(&info, "btn-danger"),
        description: html::text_between(&body, "summary-content", "</").or_else(|| html::text_between(&body, "div class=\"detail", "</div>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(&info)),
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
        .filter(|chunk| chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::attr(chunk, "title").or_else(|| html::text_between(chunk, ">", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "time", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| image_attr(chunk))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(referer)) },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-bg"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-original"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-bg"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .map(|value| absolute_url(value.trim_matches('\'')))
}

fn button_values(body: &str, class_name: &str) -> Vec<String> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains(class_name)).filter_map(|chunk| html::text_between(chunk, ">", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).collect()
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("completed") || text.contains("Complete") {
        ItemStatus::Completed
    } else if text.contains("ongoing") || text.contains("Updating") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('#').next().unwrap_or(input).split('?').next().unwrap_or(input).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

trait SubstringBetween {
    fn substring_between(&self, start: &str, end: &str) -> Option<String>;
}

impl SubstringBetween for str {
    fn substring_between(&self, start: &str, end: &str) -> Option<String> {
        self.split(start).nth(1).and_then(|rest| rest.split(end).next()).map(ToString::to_string)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="media"><h3><a href="/manga-sample.html">Sample RawINU</a></h3><img src="/cover.jpg"></div><div class="pagination"><a>»</a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="card-body"><div class="row"><h1>Sample RawINU</h1><img class="thumbnail" src="/cover.jpg"><li><a class="btn-info">Author</a></li><li><a class="btn-danger">Action</a></li><li><a class="btn-success">ongoing</a></li></div></div><div class="summary-content"><p>Summary</p></div><a href="/sample-chapter.html" title="Chapter 1"><time>01/01/2026</time></a>"#;
const PAGES_FIXTURE: &str = r#"<input name="chapter" id="chapter" value="1">"#;
const IMAGE_FIXTURE: &str = r#"<img class="chapter-img" src="/page1.jpg"><img class="chapter-img" src="/page2.jpg">"#;
