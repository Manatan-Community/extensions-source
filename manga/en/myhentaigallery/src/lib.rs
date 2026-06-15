use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MyHentaiGallery = MyHentaiGallery;
const BASE_URL: &str = "https://myhentaigallery.com";

struct MyHentaiGallery;

impl MangaSource for MyHentaiGallery {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "gpage"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}/{page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);

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
        if let Some(id) = query.strip_prefix("id:").filter(|value| !value.is_empty()) {
            let key = format!("/g/{id}");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }

        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/search/{page}?query={}",
                url::query_escape(query)
            )
        } else if let Some(artist) = filter_text(&request, "artist") {
            format!("{BASE_URL}/a/artist/{}/{page}", tag_id(&artist))
        } else if let Some(parody) = filter_text(&request, "parody") {
            format!("{BASE_URL}/a/parody/{}/{page}", tag_id(&parody))
        } else if let Some(category) =
            filter_text(&request, "category").filter(|value| !value.is_empty())
        {
            format!("{BASE_URL}/g/category/{category}/{page}")
        } else {
            let sort = filter_text(&request, "sort").unwrap_or_else(|| "gpage".to_string());
            format!("{BASE_URL}/{sort}/{page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".to_string());
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/g/sample".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("comic-inner")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(chunk, "<h2", "</h2>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Comic".to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("li.next") || body.contains("class=\"next\""),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/g/sample".to_string());
    let artists = link_values(body, "/artist/");
    let parodies = link_values(body, "/parody/");
    let groups = link_values(body, "/group/");
    let mut description = Vec::new();
    if !groups.is_empty() {
        description.push(format!("Groups:\n- {}", groups.join("\n- ")));
    }
    if !parodies.is_empty() {
        description.push(format!("Parodies:\n- {}", parodies.join("\n- ")));
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Comic".to_string()),
        cover: html::attr_after(body, "comic-listing", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: (!description.is_empty()).then(|| description.join("\n\n")),
        authors: artists.clone(),
        artists,
        tags: link_values(body, "/g/category/")
            .into_iter()
            .chain(parodies)
            .collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .map(|image| {
            image
                .replace("/thumbnail/", "/original/")
                .replace(' ', "%20")
        })
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn filter_text(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn tag_id(input: &str) -> String {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="comic-inner"><a href="/g/sample"><img src="/thumbnail/sample-001.jpg"><h2>Sample Gallery</h2></a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="comic-header"><h1>Sample Gallery</h1><div>categories <a href="/g/category/3">3D Comic</a></div><div>artists <a href="/artist/12">Artist</a></div><div>parodies <a href="/parody/34">Parody</a></div></div>
<div class="comic-listing"><div class="comic-inner"><img src="/thumbnail/sample-001.jpg"></div></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div class="comic-thumb"><img src="/thumbnail/sample-001.jpg"></div><div class="comic-thumb"><img src="/thumbnail/sample-002.jpg"></div>
"#;
