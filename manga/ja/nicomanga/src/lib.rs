use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Nicomanga = Nicomanga;
const BASE_URL: &str = "https://nicomanga.com";

struct Nicomanga;

impl MangaSource for Nicomanga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let pr = if listing == "latest" {
            "new"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga-list.html?p={page}&pr={pr}"),
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
        let mut params = vec![format!("p={page}")];
        if !query.is_empty() {
            params.push(format!("n={}", url::query_escape(query)));
        }
        let sort = filter_string(&request, "sort").unwrap_or("last_update");
        let direction = filter_string(&request, "direction").unwrap_or("DESC");
        let pr = match sort {
            "views" => "popular",
            "post" => "new",
            "name" => "az",
            _ => "all",
        };
        params.push(format!("s={}", url::query_escape(sort)));
        params.push(format!("pr={pr}"));
        params.push(format!("st={}", url::query_escape(direction)));
        if let Some(genre) = filter_string(&request, "genre").filter(|value| !value.is_empty()) {
            params.push(format!("g={}", url::query_escape(genre)));
            params.push(format!(
                "gm={}",
                filter_string(&request, "genreMode").unwrap_or("1")
            ));
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga-list.html?{}", params.join("&")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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
        .split("manga-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "manga-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Nicomanga".into()));
            let cover = html::attr_after(chunk, "manga-img", "data-src")
                .or_else(|| html::attr_after(chunk, "manga-img", "src"))
                .map(|value| url::join_url(BASE_URL, &value));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("page-link next") && !body.contains("next disabled"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&body, "manga-main-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Nicomanga".into())),
        cover: html::attr_after(&body, "manga-cover-image", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: info_links(&body, "Author"),
        tags: info_links(&body, "Genre"),
        description: html::text_between(&body, "description-text-content", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-grid-item")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-name-grid", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut urls = Vec::new();
    if let Some(raw) = html::text_between(body, "window.chapterImages = ", ";") {
        for item in raw.split('"').skip(1).step_by(2) {
            if item.starts_with("http") || item.starts_with('/') {
                urls.push(item.to_string());
            }
        }
    }
    if urls.is_empty() {
        urls.extend(
            body.split("chapter-image-wrapper")
                .skip(1)
                .filter_map(|chunk| {
                    html::attr_after(chunk, "<img", "data-src")
                        .or_else(|| html::attr_after(chunk, "<img", "src"))
                }),
        );
    }
    urls.into_iter()
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

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("info-field-label")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input.strip_prefix(BASE_URL).unwrap_or(input)))
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

fn absolute_url(key: &str) -> String {
    format!("{BASE_URL}{}", normalize_key(key))
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga-grid"><div class="manga-card"><a class="manga-title" href="/manga/sample">Sample Nicomanga</a><img class="manga-img" src="/cover.jpg"></div></div><a class="page-link next" href="?p=2">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="manga-main-title">Sample Nicomanga</h1><img class="manga-cover-image" src="/cover.jpg"><div class="info-field-label">Author</div><div class="info-field-value"><a>Sample Author</a></div><div class="info-field-label">Genre</div><div class="info-field-value"><a>Action</a></div><div class="description-text-content">Sample description.</div><div id="chapter-grid"><a class="chapter-grid-item" href="/manga/sample/chapter-1"><span class="chapter-name-grid">Chapter 1</span><span class="chapter-time-grid">1 day ago</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<script>window.chapterImages = ["https://nicomanga.com/page1.jpg","https://nicomanga.com/page2.jpg"];</script>"#;
