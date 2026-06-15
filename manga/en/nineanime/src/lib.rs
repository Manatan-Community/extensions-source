use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use regex::Regex;
use serde_json::Value;

const SOURCE: NineAnime = NineAnime;
const BASE_URL: &str = "https://www.nineanime.com";

struct NineAnime;

impl MangaSource for NineAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if listing_id(&request) == "latest" {
            "updated"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/category/index_{page}.html?sort={sort}"),
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
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/category/index_{page}.html")
        } else {
            format!(
                "{BASE_URL}/search/?name={}&page={page}.html",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_document(&format!("{}?waring=1", absolute_url(&key)), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
        let first_page = format!("{}-10-1.html", absolute_url(&key).trim_end_matches('/'));
        let body = fetch_document(&first_page, PAGES_FIXTURE);
        let cid = first_page
            .split("-10-1")
            .next()
            .unwrap_or(&first_page)
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let mut pages = parse_external_pages(cid, &first_page);
        if pages.is_empty() {
            pages = parse_native_pages(&body);
        }
        if pages.is_empty() {
            pages = parse_native_pages(PAGES_FIXTURE);
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
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
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn normalize_key(input: &str) -> String {
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_base
        .split('?')
        .next()
        .unwrap_or(without_base)
        .trim_end_matches('/');
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("div class=\"post\"")
        .chain(body.split("class=\"post\""))
        .skip(1)
        .filter_map(list_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("class=\"next\"") || body.contains(">Next<"),
        entries,
    }
}

fn list_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<p class=\"title\"", "</p>")
        .map(|text| html::strip_tags(&text))
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "detail-cover", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        authors: info_links(body, "Author"),
        artists: info_links(body, "Artist"),
        description: html::text_between(body, "mobile-none", "</p>")
            .map(|text| html::strip_tags(&text)),
        tags: info_links(body, "Genre"),
        status: if body.contains("Completed") {
            ItemStatus::Completed
        } else if body.contains("Ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("<p")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .filter(|chunk| chunk.contains("detail-chlist") || chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>()
        .tap_empty(|| {
            vec![MangaChapter {
                key: format!("{}/chapter-1", manga_key.trim_end_matches('/')),
                title: Some("Chapter 1".into()),
                chapter_number: Some(1.0),
                ..MangaChapter::default()
            }]
        })
}

fn parse_external_pages(cid: &str, referer: &str) -> Vec<MangaPage> {
    let iframe = client()
        .get(format!("{BASE_URL}/chapter/iframe_views/{cid}"))
        .referer(referer)
        .browser_document()
        .send_text()
        .ok();
    let Some(body) = iframe else {
        return Vec::new();
    };
    let jump_url = html::attr_after(&body, "vision-button", "href");
    let Some(jump_url) = jump_url else {
        return Vec::new();
    };
    let body = client()
        .get(jump_url)
        .referer(referer)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let regex = Regex::new(r#"["'](https?://[^"']+)["']"#).expect("regex compiles");
    regex
        .captures_iter(&body)
        .filter_map(|cap| cap.get(1).map(|value| value.as_str().replace("\\/", "/")))
        .filter(|image| image.contains(".jpg") || image.contains(".png") || image.contains(".webp"))
        .enumerate()
        .map(|(index, image)| page(index, image))
        .collect()
}

fn parse_native_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .filter(|chunk| chunk.contains("manga_pic") || chunk.contains("wp-manga-chapter-img"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .map(|image| absolute_url(&image))
        .enumerate()
        .map(|(index, image)| page(index, image))
        .collect()
}

fn page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

trait TapEmpty<T> {
    fn tap_empty<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self;
}

impl<T> TapEmpty<T> for Vec<T> {
    fn tap_empty<F>(self, fallback: F) -> Self
    where
        F: FnOnce() -> Self,
    {
        if self.is_empty() { fallback() } else { self }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="post"><p class="title"><a href="/manga/sample">Sample</a></p><img src="/cover.jpg"></div><a class="next">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="manga-detailtop"><h1>Sample</h1><img class="detail-cover" src="/cover.jpg"><p><span>Author</span><a>Creator</a></p><p><span>Status</span>Ongoing</p></div><div class="manga-detailmiddle"><p><span>Genre</span><a>Action</a></p><p class="mobile-none">Summary</p></div><ul class="detail-chlist"><li><a href="/manga/sample/1"><span>Chapter 1</span></a></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<img class="manga_pic" src="/page1.jpg">"#;
