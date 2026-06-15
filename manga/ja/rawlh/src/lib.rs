use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: WeLoveManga = WeLoveManga;
const BASE_URL: &str = "https://weloma.art";
const LIST_PATH: &str = "manga-list.html";

struct WeLoveManga;

impl MangaSource for WeLoveManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last_update"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &list_url(page, "", sort, ""),
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let sort = filter_string(&request, "sort").unwrap_or("views");
        let status = filter_string(&request, "status").unwrap_or_default();
        Ok(parse_listing(&fetch_document(
            &list_url(page(&request), query, sort, status),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
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
        Ok(parse_pages(
            &fetch_document(&absolute_url(&key), PAGES_FIXTURE),
            &absolute_url(&key),
        ))
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
                item: key.contains("/manga/").then(|| details_by_key(&key)),
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

fn list_url(page: u64, query: &str, sort: &str, status: &str) -> String {
    let mut parts = vec![
        "listType=pagination".to_string(),
        format!("page={page}"),
        format!("sort={}", url::query_escape(sort)),
        "sort_type=DESC".to_string(),
    ];
    if !query.is_empty() {
        parts.push(format!("name={}", url::query_escape(query)));
    }
    if !status.is_empty() {
        parts.push(format!("m_status={}", url::query_escape(status)));
    }
    format!("{BASE_URL}/{LIST_PATH}?{}", parts.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("media") || chunk.contains("thumb-item-flow"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "series-title", "href")
                .or_else(|| html::attr_after(chunk, "<h3", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "series-title", "</")
                    .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "WeLoveManga".into())
                    }),
                cover: background_image(chunk)
                    .or_else(|| image_attr(chunk))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination") || body.contains("btn-info"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(&absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let info = body.split("manga-info").nth(1).unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "WeLoveManga".into())),
        cover: html::attr_after(body, "thumbnail", "src")
            .or_else(|| image_attr(info))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "summary-content", "</")
            .or_else(|| html::text_between(body, "div class=\"detail\"", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: link_texts(info, "btn-info"),
        tags: link_texts(info, "btn-danger"),
        status: parse_status(&html::strip_tags(info)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split(['\n', '\r'])
        .flat_map(|line| line.split("<p").skip(1).collect::<Vec<_>>())
        .chain(body.split("<tr").skip(1))
        .chain(body.split("list-chapters").skip(1))
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "<a", "</a>")
                        .or_else(|| html::attr(chunk, "title"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                date_uploaded: html::text_between(chunk, "<time", "</time>")
                    .or_else(|| html::text_between(chunk, "chapter-time", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), |mut chapters, chapter| {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
            chapters
        })
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-img")
                .or_else(|| html::attr(chunk, "data-original"))
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-srcset"))
                .or_else(|| html::attr(chunk, "data-aload"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter_map(|image| decode_image_attr(&image))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_image_attr(input: &str) -> Option<String> {
    if input.contains('.') {
        return Some(input.trim().to_string()).filter(|value| !value.starts_with("data:"));
    }
    decode_base64(input.trim())
}

fn decode_base64(input: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("completed") || lower.contains("complete") || lower.contains("完結") {
        ItemStatus::Completed
    } else if lower.contains("ongoing") || lower.contains("updating") || lower.contains("連載") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn link_texts(body: &str, class_name: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(class_name))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("Updating"))
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-bg"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn background_image(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "content img-in-ratio", "style")
        .or_else(|| html::attr_after(chunk, "img-in-ratio", "style"))
        .and_then(|style| {
            style
                .split("url(")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
                .map(|value| value.trim_matches(['\'', '"']).to_string())
        })
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
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

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="media"><h3><a href="/manga/sample">Sample WeLoveManga</a></h3><div class="content img-in-ratio" style="background-image: url('/cover.jpg')"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"
<div class="manga-info"><h1>Sample WeLoveManga</h1><img class="thumbnail" src="/cover.jpg"><li><a class="btn-info">Author</a></li><li><a class="btn-danger">Action</a></li><li><a class="btn-success">Ongoing</a></li></div>
<div class="summary-content"><p>Sample description.</p></div>
<div id="list-chapters"><p><a href="/manga/sample/chapter-1">Chapter 1</a><time>2024-01-01</time></p></div>
"#;
const PAGES_FIXTURE: &str = r#"<img class="chapter-img" data-img="L3BhZ2UxLmpwZw=="><img class="chapter-img" src="/page2.jpg">"#;
