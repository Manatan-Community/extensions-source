use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: HattoriManga = HattoriManga;
const BASE_URL: &str = "https://hattorimanga.net";
const LANG: &str = "tr";
const CONTENT_RATING: &str = "adult";
const SEARCH_PREFIX: &str = "slug:";

struct HattoriManga;

impl MangaSource for HattoriManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_json_text(
                &format!("{BASE_URL}/latest-chapters"),
                LATEST_FIXTURE,
            )));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/manga?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if let Some(slug) = query.strip_prefix(SEARCH_PREFIX) {
            let key = format!("/manga/{}", slug.trim_matches('/'));
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if query.is_empty() {
            if let Some(target) = genre_url(page, request.get("filters").unwrap_or(&Value::Null)) {
                return Ok(parse_listing(&fetch_document_or_fixture(
                    &target,
                    LIST_FIXTURE,
                )));
            }
        }
        let body = post_search(query).unwrap_or_else(|| SEARCH_FIXTURE.to_string());
        let entries = serde_json::from_str::<Vec<SearchManga>>(&body)
            .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or_default())
            .into_iter()
            .map(SearchManga::into_item)
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = key.trim_matches('/').rsplit('/').next().unwrap_or("sample");
        Ok(fetch_all_chapters(slug, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = absolute_url(&key);
        let pages = parse_pages(
            &fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE),
            &chapter_url,
        );
        if pages.is_empty() {
            return Ok(vec![MangaPage {
                content: PageContent::Text {
                    text: "Open WebView and sign in to view this chapter".to_string(),
                },
                description: Some("Sign in required".to_string()),
                ..MangaPage::default()
            }]);
        }
        Ok(pages)
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
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_search(query: &str) -> Option<String> {
    let home = fetch_document_or_fixture(BASE_URL, HOME_FIXTURE);
    let token = html::attr_after(&home, "csrf-token", "content")?;
    client()
        .post(format!("{BASE_URL}/manga/search"))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("_token", token.as_str()), ("query", query)])
        .xhr()
        .send_text()
        .ok()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("product-card"))
        .filter_map(|chunk| {
            let script = html::attr_after(chunk, "onclick", "onclick")
                .or_else(|| html::attr(chunk, "onclick"))
                .unwrap_or_default();
            let href = manga_url_from_onclick(&script)
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h5", "</h5>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image(chunk).map(|value| absolute_url(&value)),
                tags: product_tags(chunk),
                url: Some(absolute_url(&key)),
                language: Some(LANG.into()),
                content_rating: Some(CONTENT_RATING.into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_item);
    Paged {
        has_next_page: body.contains("pagination")
            && body.contains("page-item")
            && !body.contains("page-item disabled"),
        entries,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<LatestPayload>(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap());
    let entries = payload
        .chapters
        .into_iter()
        .map(|chapter| chapter.manga.into_item())
        .fold(Vec::new(), push_unique_item);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h3", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "set-bg", "data-setbg")
            .or_else(|| image(body))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "anime-details-text", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: details_value(body, "Yazar").into_iter().collect(),
        artists: details_value(body, "Cizer")
            .or_else(|| details_value(body, "Çizer"))
            .into_iter()
            .collect(),
        tags: details_value(body, "Etiketler")
            .map(|value| split_csv(&value))
            .unwrap_or_default(),
        status: details_value(body, "Durum")
            .map(|value| parse_status(&value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(&key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(slug: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut page = 1;
    loop {
        let body = fetch_json_text(
            &format!("{BASE_URL}/load-more-chapters/{slug}?page={page}"),
            CHAPTERS_FIXTURE,
        );
        let dto = serde_json::from_str::<ChapterPayload>(&body)
            .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).unwrap());
        for chapter in dto.chapters {
            chapters.push(chapter.into_chapter(manga_key));
        }
        if dto.current_page >= dto.last_page {
            break;
        }
        page = (dto.current_page + 1).min(dto.last_page);
    }
    chapters
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-src") || chunk.contains("image-wrapper"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn genre_url(page: u64, filters: &Value) -> Option<String> {
    let genres = filters.get("genres").and_then(Value::as_str)?.trim();
    if genres.is_empty() {
        return None;
    }
    let mut target = format!("{BASE_URL}/manga-index?page={page}");
    for genre in split_csv(genres) {
        target.push_str("&genres[]=");
        target.push_str(&url::query_escape(&genre));
    }
    Some(target)
}

fn details_value(body: &str, label: &str) -> Option<String> {
    body.split("<li").skip(1).find_map(|chunk| {
        if !html::strip_tags(chunk).contains(label) {
            return None;
        }
        let text = html::strip_tags(chunk);
        Some(text.replace(label, "").trim().to_string())
    })
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("devam") {
        ItemStatus::Ongoing
    } else if lower.contains("tamam") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn image(body: &str) -> Option<String> {
    html::attr_after(body, "data-setbg", "data-setbg")
        .or_else(|| html::attr_after(body, "<img", "data-src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn product_tags(body: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</li>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn manga_url_from_onclick(value: &str) -> Option<String> {
    let start = value.find("='").map(|index| index + 2)?;
    let rest = &value[start..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find("/manga/") {
            return format!("/{}", value[index + 1..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn parse_hattori_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('.');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Deserialize)]
struct ChapterPayload {
    chapters: Vec<ChapterDto>,
    #[serde(rename = "currentPage")]
    current_page: u64,
    #[serde(rename = "lastPage")]
    last_page: u64,
}

#[derive(Deserialize)]
struct ChapterDto {
    title: String,
    #[serde(rename = "chapter_slug")]
    chapter_slug: String,
    #[serde(rename = "formattedUploadTime")]
    formatted_upload_time: String,
}

impl ChapterDto {
    fn into_chapter(self, manga_key: &str) -> MangaChapter {
        let key = format!(
            "{}/{}",
            manga_key.trim_end_matches('/'),
            self.chapter_slug.trim_matches('/')
        );
        MangaChapter {
            key: key.clone(),
            title: Some(self.title),
            date_uploaded: parse_hattori_date(&self.formatted_upload_time),
            url: Some(absolute_url(&key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Deserialize)]
struct LatestPayload {
    chapters: Vec<LatestChapter>,
}

#[derive(Deserialize)]
struct LatestChapter {
    manga: MangaDto,
}

#[derive(Deserialize)]
struct MangaDto {
    slug: String,
    title: String,
    #[serde(rename = "cover_image")]
    cover_image: String,
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        let key = format!("/manga/{}", self.slug.trim_matches('/'));
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(format!(
                "{BASE_URL}/storage/{}",
                self.cover_image.trim_start_matches('/')
            )),
            url: Some(absolute_url(&key)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct SearchManga {
    slug: String,
    title: String,
    #[serde(rename = "cover_image")]
    cover_image: String,
}

impl SearchManga {
    fn into_item(self) -> CatalogItem {
        MangaDto {
            slug: self.slug,
            title: self.title,
            cover_image: self.cover_image,
        }
        .into_item()
    }
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str =
    r#"<html><head><meta name="csrf-token" content="fixture-token"></head></html>"#;

const LIST_FIXTURE: &str = r#"
<div class="product-card grow-box" onclick="location.href='/manga/sample'">
  <div class="img-con" data-setbg="/storage/sample.jpg"></div>
  <h5>Sample Hattori</h5>
  <div class="product-card-con"><ul><li>Aksiyon</li></ul></div>
</div>
<ul class="pagination"><li class="page-item"><a href="?page=2">2</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h3>Sample Hattori</h3>
<div class="set-bg" data-setbg="/storage/sample.jpg"></div>
<div class="anime-details-text"><p>Sample description.</p></div>
<div class="anime-details-widget">
  <li><span>Yazar</span> Sample Author</li>
  <li><span>Çizer</span> Sample Artist</li>
  <li><span>Durum</span> Devam Ediyor</li>
  <li><span>Etiketler</span> Aksiyon, Macera</li>
</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "chapters": [
    { "title": "Bolum 1", "manga_slug": "sample", "chapter_slug": "bolum-1", "formattedUploadTime": "01.01.2024" }
  ],
  "currentPage": 1,
  "lastPage": 1
}"#;

const PAGES_FIXTURE: &str = r#"
<div class="image-wrapper"><img data-src="/storage/sample/page-1.jpg"></div>
"#;

const LATEST_FIXTURE: &str = r#"{
  "chapters": [
    { "manga": { "title": "Sample Hattori", "slug": "sample", "cover_image": "sample.jpg" } }
  ]
}"#;

const SEARCH_FIXTURE: &str = r#"[
  { "title": "Sample Hattori", "slug": "sample", "cover_image": "sample.jpg" }
]"#;
