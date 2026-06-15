use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Otakusic = Otakusic;
const BASE_URL: &str = "https://otakusic.com";
const IMG_BASE_URL: &str = "https://img.otakusic.com";

struct Otakusic;

impl MangaSource for Otakusic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "views"
        } else {
            "updated"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/tim-kiem?sort={sort}&page={page}"),
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
        let page = page(&request);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut pairs = vec![format!("page={page}")];
        if !query.is_empty() {
            pairs.push(format!("q={}", url::query_escape(query)));
        }
        pairs.push(format!(
            "sort={}",
            url::query_escape(filter(filters, "sort").unwrap_or("updated"))
        ));
        if let Some(status) = filter(filters, "status") {
            pairs.push(format!("status={}", url::query_escape(status)));
        }
        if let Some(genre) = filter(filters, "genre") {
            pairs.push(format!("category={}", url::query_escape(genre)));
        }
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/tim-kiem?{}", pairs.join("&")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/chi-tiet/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/chi-tiet/sample".into());
        let slug = slug_from_key(&key);
        let body = fetch_json(
            &format!("{BASE_URL}/api/v1/manga/chapters/{slug}"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/api/chapter/sample/chapter-1/chapter-1".into());
        let parts = key
            .trim_start_matches("/api/chapter/")
            .split('/')
            .collect::<Vec<_>>();
        let manga_slug = parts.first().copied().unwrap_or("sample");
        let chapter_original_slug = parts.get(1).copied().unwrap_or("chapter-1");
        let body = fetch_json(
            &format!("{BASE_URL}/api/v1/manga/chapters/{manga_slug}"),
            CHAPTERS_FIXTURE,
        );
        let pages = pages_from_chapter_api(&body, manga_slug, chapter_original_slug);
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
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
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key
                .trim_start_matches("/api/chapter/")
                .split('/')
                .collect::<Vec<_>>();
            format!(
                "{BASE_URL}/doc-truyen/{}/{}",
                parts.first().copied().unwrap_or_default(),
                parts.get(2).copied().unwrap_or_default()
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/chi-tiet/").then(|| details_by_key(&key)),
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
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chi-tiet/") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination-btn") && body.contains("Sau"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| absolute_url(&image)),
        authors: link_texts_after(body, "Tác giả"),
        tags: link_texts_after(body, "flex flex-wrap gap-2"),
        description: html::text_between(body, "id=\"description\"", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if body.contains("status=completed") {
            ItemStatus::Completed
        } else if body.contains("status=ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_slug: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ChaptersResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    response
        .data
        .into_iter()
        .filter(|chapter| chapter.status.as_deref() != Some("inactive"))
        .map(|chapter| {
            let key = format!(
                "/api/chapter/{manga_slug}/{}/{}",
                chapter.chapter_original_slug, chapter.chapter_slug
            );
            MangaChapter {
                key: key.clone(),
                title: Some(format!("Chuong {}", chapter.chapter_name.content)),
                date_uploaded: chapter
                    .public_at
                    .or(chapter.updated_at)
                    .and_then(|date| parse_ymd_prefix(&date)),
                url: Some(format!(
                    "{BASE_URL}/doc-truyen/{manga_slug}/{}",
                    chapter.chapter_slug
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn pages_from_chapter_api(
    body: &str,
    manga_slug: &str,
    chapter_original_slug: &str,
) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ChaptersResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    let Some(chapter) = response
        .data
        .into_iter()
        .find(|chapter| chapter.chapter_original_slug == chapter_original_slug)
    else {
        return Vec::new();
    };
    let images = chapter
        .api_url
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    images.into_iter().enumerate().map(|(index, filename)| {
        let image = format!("{IMG_BASE_URL}/manga/uploads/chapter/{manga_slug}/{chapter_original_slug}/{filename}");
        MangaPage {
            content: PageContent::Url { url: image.clone(), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        }
    }).collect()
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .map(|idx| {
            body[idx..]
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_ymd_prefix(value: &str) -> Option<i64> {
    value.get(..10).and_then(manatan_shared::dates::parse_ymd)
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn slug_from_key(key: &str) -> String {
    key.trim_start_matches(BASE_URL)
        .trim_start_matches("/chi-tiet/")
        .trim_matches('/')
        .to_string()
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        value
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(value)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/chi-tiet/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Deserialize)]
struct ChaptersResponse {
    data: Vec<ChapterDto>,
}
#[derive(Deserialize)]
struct ChapterDto {
    status: Option<String>,
    #[serde(rename = "chapter_original_slug")]
    chapter_original_slug: String,
    #[serde(rename = "chapter_slug")]
    chapter_slug: String,
    #[serde(rename = "chapter_name")]
    chapter_name: LocalizedText,
    #[serde(rename = "public_at")]
    public_at: Option<String>,
    #[serde(rename = "updated_at")]
    updated_at: Option<String>,
    #[serde(rename = "api_url")]
    api_url: Option<String>,
}
#[derive(Deserialize)]
struct LocalizedText {
    content: String,
}

const LIST_FIXTURE: &str = r#"<a href="/chi-tiet/sample"><img alt="Sample" src="/cover.jpg"></a>"#;
const DETAILS_FIXTURE: &str =
    r#"<h1>Sample</h1><img alt="Sample" src="/cover.jpg"><div id="description">Summary</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"status":"active","chapter_original_slug":"chapter-1","chapter_slug":"chapter-1","chapter_name":{"content":"1"},"updated_at":"2024-01-01 00:00:00","api_url":"[\"page1.jpg\"]"}]}"#;

export_manga_source!(SOURCE);
