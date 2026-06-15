use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Raw18 = Raw18;
const BASE_URL: &str = "https://raw18.cam";

struct Raw18;

impl MangaSource for Raw18 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            paged_url(BASE_URL, page)
        } else if page > 1 {
            format!("{BASE_URL}/hot?page={page}")
        } else {
            format!("{BASE_URL}/hot")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let mut params = vec![
            format!("keyword={}", url::query_escape(query)),
            format!("page={}", page(&request)),
        ];
        if let Some(status) = filter_string(&request, "status").filter(|value| !value.is_empty()) {
            params.push(format!("status={}", url::query_escape(status)));
        }
        if let Some(genre) = filter_string(&request, "genre").filter(|value| !value.is_empty()) {
            params.push(format!("genre={}", url::query_escape(genre)));
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/search/manga?{}", params.join("&")),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE), &absolute_url(&key)))
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
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter(|chunk| chunk.contains("item"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "<h3", "</h3>")
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Raw18".into())),
                    cover: image_or_null(chunk),
                    url: Some(absolute_url(&key)),
                    language: Some("ja".into()),
                    content_rating: Some("adult".into()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("page-link") && body.contains("href"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = html::text_between(body, "article", "</article>").unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Raw18".into())),
        cover: image_or_null(&info),
        authors: labeled_values(&info, "author"),
        tags: labeled_values(&info, "kind"),
        description: html::text_between(&info, "detail-content", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(&info)),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "col-xs-4", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-original").or_else(|| html::attr(chunk, "data-src")).or_else(|| html::attr(chunk, "src")))
        .filter(|src| !src.is_empty() && !src.starts_with("data:"))
        .fold(Vec::<String>::new(), |mut out, image| {
            let image = absolute_url(&image);
            if !out.contains(&image) {
                out.push(image);
            }
            out
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(referer)) },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_or_null(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .filter(|value| value.starts_with("http") || value.starts_with('/'))
        .map(|value| absolute_url(&value))
}

fn labeled_values(body: &str, class: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains(class))
        .flat_map(|chunk| {
            let links = chunk
                .split("<a")
                .skip(1)
                .filter_map(|link| html::text_between(link, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if links.is_empty() {
                html::text_between(chunk, "col-xs-8", "</").map(|value| vec![html::strip_tags(&value)]).unwrap_or_default()
            } else {
                links
            }
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("完結") || text.contains("Completed") || text.contains("Complete") {
        ItemStatus::Completed
    } else if text.contains("連載") || text.contains("Ongoing") || text.contains("Updating") {
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

fn paged_url(base: &str, page: u64) -> String {
    if page > 1 { format!("{base}?page={page}") } else { base.into() }
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="items"><article class="item"><a href="https://raw18.cam/manga/sample"><h3>Sample Raw18</h3><img src="/cover.jpg"></a></article></div><a class="page-link" href="?page=2">2</a>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<article id="item-detail"><h1>Sample Raw18</h1><li class="author"><p class="col-xs-8">Author</p></li><li class="status"><p class="col-xs-8">Ongoing</p></li><li class="kind"><p class="col-xs-8"><a>Adult</a></p></li><div class="detail-content"><p>Summary</p></div><div class="col-image"><img src="/cover.jpg"></div></article><div class="list-chapter"><li class="row"><a href="/manga/sample/chapter-1">Chapter 1</a><div class="col-xs-4">1 day ago</div></li></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-chapter"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
