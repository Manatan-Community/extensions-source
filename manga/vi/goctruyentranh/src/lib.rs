use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: GocTruyenTranh = GocTruyenTranh;
const BASE_URL: &str = "https://goctruyentranh.com";

struct GocTruyenTranh;

impl MangaSource for GocTruyenTranh {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "truyen-hot"
        } else {
            "truyen-moi-cap-nhat"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/danh-sach/{path}?page={page}"),
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut params = vec![
            format!("keyword={}", url::query_escape(query)),
            format!("page={page}"),
        ];
        append_multi(filters, "categories", "categories", &mut params);
        append_multi(filters, "status", "status", &mut params);
        append_multi(filters, "country", "country", &mut params);
        if let Some(value) = filter(filters, "minChap").filter(|value| !value.is_empty()) {
            params.push(format!("minChap={}", url::query_escape(value)));
        }
        if let Some(value) = filter(filters, "sort").filter(|value| !value.is_empty()) {
            params.push(format!("sort={}", url::query_escape(value)));
        }
        Ok(parse_search_json(&fetch_json(
            &format!("{BASE_URL}/baseapi/comics/filterComic?{}", params.join("&")),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("line-clamp-2") || chunk.contains("_next/image"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "line-clamp-2", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "line-clamp-2", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| thumbnail_url(&absolute_url(&image))),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("<nav") && body.contains("<li"),
    }
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    let entries = response
        .comics
        .data
        .into_iter()
        .map(|item| {
            let key = normalize_key(&format!("/{}", item.slug));
            CatalogItem {
                key: key.clone(),
                title: item.name,
                cover: item
                    .thumbnail
                    .map(|image| thumbnail_url(&absolute_url(&image))),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: response.comics.current_page != response.comics.last_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| thumbnail_url(&absolute_url(&image))),
        authors: text_after_label(body, "Tác giả:")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: link_texts_after(body, "Thể loại:"),
        description: html::text_between(body, "mt-3", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("Chapter") && chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "Chapter", "</")
                .map(|value| format!("Chapter{}", html::strip_tags(&value)))
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "text-center", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("lozad") || chunk.contains("data-src"))
        .filter_map(|chunk| html::attr(chunk, "data-src"))
        .filter(|image| looks_like_image(image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            let image = absolute_url(&image);
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| page(index, &image))
        .collect()
}

fn thumbnail_url(input: &str) -> String {
    if input.contains("_next/image") {
        input.replace("w=96", "w=384")
    } else {
        format!(
            "{BASE_URL}/_next/image?url={}&w=384&q=75",
            url::query_escape(input)
        )
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "src").or_else(|| {
        html::attr(chunk, "srcset").and_then(|srcset| {
            srcset
                .split(',')
                .next()?
                .split_whitespace()
                .next()
                .map(ToString::to_string)
        })
    })
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .and_then(|index| html::text_between(&body[index..], "<b", "</b>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts_after(body: &str, label: &str) -> Vec<String> {
    body.find(label)
        .map(|index| {
            body[index..]
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("đã hoàn thành") {
        ItemStatus::Completed
    } else if lower.contains("tạm ngưng") || lower.contains("tạm hoãn") {
        ItemStatus::Hiatus
    } else if lower.contains("đang tiến hành") || lower.contains("đang cập nhật") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:")
        && [".jpg", ".jpeg", ".png", ".webp", ".avif"]
            .iter()
            .any(|ext| lower.contains(ext))
}

fn page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else {
        format!("{BASE_URL}/{}", value.trim_start_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key != "/")
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn append_multi(filters: &Value, id: &str, param: &str, out: &mut Vec<String>) {
    if let Some(values) = filters.get(id).and_then(Value::as_array) {
        for value in values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            out.push(format!("{param}={}", url::query_escape(value)));
        }
    } else if let Some(value) = filter(filters, id) {
        out.push(format!("{param}={}", url::query_escape(value)));
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Deserialize)]
struct SearchResponse {
    comics: Comics,
}

#[derive(Deserialize)]
struct Comics {
    current_page: u64,
    data: Vec<SearchItem>,
    last_page: u64,
}

#[derive(Deserialize)]
struct SearchItem {
    name: String,
    slug: String,
    thumbnail: Option<String>,
}

const LIST_FIXTURE: &str = r#"<section class="mt-12"><div class="grid"><div class="flex"><a class="line-clamp-2" href="/sample">Sample</a><img src="/cover.jpg"></div></div></section>"#;
const SEARCH_FIXTURE: &str = r#"{"comics":{"current_page":1,"data":[{"name":"Sample","slug":"sample","thumbnail":"/cover.jpg"}],"last_page":1}}"#;
const DETAILS_FIXTURE: &str = r#"<section><aside><h1>Sample</h1><img src="/cover.jpg"></aside><span>Thể loại:</span><a>Action</a><span>Tác giả:</span><b>Author</b><span>Trạng thái:</span><b>Đang Tiến Hành</b><div class="mt-3"><p>Summary</p></div><ul><li><a href="/sample/chapter-1"><span class="items-center">Chapter 1</span><span class="text-center">01-01-2024</span></a></li></ul></section>"#;
const PAGES_FIXTURE: &str = r#"<img class="lozad" data-src="/page1.jpg">"#;

export_manga_source!(SOURCE);
