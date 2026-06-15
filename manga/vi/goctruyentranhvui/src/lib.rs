use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: GocTruyenTranhVui = GocTruyenTranhVui;
const BASE_URL: &str = "https://goctruyentranhvui30.com";
const API_URL: &str = "https://goctruyentranhvui30.com/api/v2";

struct GocTruyenTranhVui;

impl MangaSource for GocTruyenTranhVui {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "viewCount"
        } else {
            "recentDate"
        };
        Ok(search_api(page, "", &[("orders[]", order)]))
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
        let mut params = Vec::new();
        append_multi(filters, "status", "status[]", &mut params);
        append_multi(filters, "orders", "orders[]", &mut params);
        append_multi(filters, "categories", "categories[]", &mut params);
        Ok(search_api(
            page,
            query,
            &params
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect::<Vec<_>>(),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1:sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1:sample".into());
        let (comic_id, slug) = split_manga_key(&key);
        let target = format!("{BASE_URL}/api/comic/{comic_id}/chapter?limit=-1");
        Ok(parse_chapters(
            &fetch_json(&target, CHAPTERS_FIXTURE),
            &slug,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chuong-1#1".into());
        let slug = key
            .split("/truyen/")
            .nth(1)
            .and_then(|tail| tail.split("/chuong-").next())
            .unwrap_or("sample");
        let chapter = key
            .split("/chuong-")
            .nth(1)
            .and_then(|tail| tail.split('#').next())
            .unwrap_or("1");
        let comic_id = key.split('#').nth(1).unwrap_or("1");
        let body = post_chapter_pages(comic_id, chapter, slug);
        Ok(parse_pages(&body))
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
        Ok(manga::request_key(&request, "manga").map(|key| {
            let (_, slug) = split_manga_key(&key);
            format!("{BASE_URL}/truyen/{slug}")
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug = key
                .split("/truyen/")
                .nth(1)
                .and_then(|tail| tail.split("/chuong-").next())
                .unwrap_or("sample");
            let chapter = key
                .split("/chuong-")
                .nth(1)
                .and_then(|tail| tail.split('#').next())
                .unwrap_or("1");
            format!("{BASE_URL}/truyen/{slug}/chuong-{chapter}")
        }))
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_chapter_pages(comic_id: &str, chapter: &str, slug: &str) -> String {
    client()
        .post(&format!("{BASE_URL}/api/chapter/loadAll"))
        .origin(BASE_URL)
        .referer(&format!("{BASE_URL}/truyen/{slug}/chuong-{chapter}"))
        .form(&[
            ("comicId", comic_id),
            ("chapterNumber", chapter),
            ("nameEn", slug),
        ])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string())
}

fn search_api(page: u64, query: &str, extra: &[(&str, &str)]) -> Paged<CatalogItem> {
    let mut params = vec![format!("p={}", page.saturating_sub(1))];
    if !query.is_empty() {
        params.push(format!("searchValue={}", url::query_escape(query)));
    }
    for (key, value) in extra {
        params.push(format!("{key}={}", url::query_escape(value)));
    }
    parse_listing_json(&fetch_json(
        &format!("{API_URL}/search?{}", params.join("&")),
        LIST_FIXTURE,
    ))
}

fn parse_listing_json(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ResultDto<ListingDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let entries = response
        .result
        .data
        .into_iter()
        .map(|item| item.catalog_item())
        .collect();
    Paged {
        entries,
        has_next_page: response.result.next,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let (_, slug) = split_manga_key(key);
    parse_details(
        &fetch_document(&format!("{BASE_URL}/truyen/{slug}"), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("const comic ="))
        .unwrap_or_default();
    let id = regex_like(script, "id:", "\"")
        .or_else(|| html::attr_after(body, "comic-id-comment", "value"));
    let slug = regex_like(script, "nameEn:", "`")
        .or_else(|| fallback_key.split(':').nth(1).map(ToString::to_string))
        .unwrap_or_else(|| "sample".into());
    let key = id
        .as_ref()
        .map(|id| format!("{id}:{slug}"))
        .unwrap_or_else(|| fallback_key.into());
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "v-card-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| slug.clone()),
        cover: html::attr_after(body, "img class=\"image\"", "src")
            .or_else(|| html::attr_after(body, "image", "src"))
            .map(|image| absolute_url(&image)),
        authors: text_after_label(body, "Tác giả:")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: link_texts_by_marker(body, "v-chip-link"),
        description: html::text_between(body, "v-card-text", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&text),
        url: Some(format!("{BASE_URL}/truyen/{slug}")),
        language: Some("vi".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<ResultDto<ChapterListDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    response
        .result
        .chapters
        .into_iter()
        .map(|chapter| {
            let key = format!(
                "/truyen/{slug}/chuong-{}#{}",
                chapter.number_chapter, chapter.comic_id
            );
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.number_chapter.clone()),
                date_uploaded: Some(chapter.update_time),
                url: Some(
                    format!("{BASE_URL}{key}")
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .to_string(),
                ),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ResultDto<ImageListDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    response
        .result
        .data
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let image = absolute_url(&image);
            page(index, &image)
        })
        .collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("end") {
        ItemStatus::Completed
    } else if lower.contains("đang thực hiện") || lower.contains("prg") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") {
        value.into()
    } else {
        format!("{BASE_URL}/{}", value.trim_start_matches('/'))
    }
}

fn split_manga_key(key: &str) -> (String, String) {
    if let Some((id, slug)) = key.split_once(':') {
        (id.into(), slug.into())
    } else {
        (
            "1".into(),
            key.trim_start_matches("/truyen/").trim_matches('/').into(),
        )
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| {
            input
                .trim_start_matches(BASE_URL)
                .trim_start_matches("/truyen/")
                .trim_matches('/')
                .to_string()
        })
        .filter(|key| !key.is_empty())
}

fn regex_like(script: &str, marker: &str, quote: &str) -> Option<String> {
    let start = script.find(marker)?;
    let tail = &script[start + marker.len()..];
    let first = tail.find(quote)?;
    let rest = &tail[first + quote.len()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .and_then(|index| html::text_between(&body[index..], "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts_by_marker(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn append_multi(
    filters: &Value,
    id: &str,
    param: &'static str,
    out: &mut Vec<(&'static str, String)>,
) {
    if let Some(values) = filters.get(id).and_then(Value::as_array) {
        for value in values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            out.push((param, value.into()));
        }
    } else if let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        out.push((param, value.into()));
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

#[derive(Deserialize)]
struct ResultDto<T> {
    result: T,
}

#[derive(Deserialize)]
struct ListingDto {
    next: bool,
    data: Vec<MangaDto>,
}

#[derive(Deserialize)]
struct MangaDto {
    id: String,
    name: String,
    description: Option<String>,
    #[serde(rename = "statusCode")]
    status_code: Option<String>,
    photo: String,
    #[serde(rename = "nameEn")]
    name_en: String,
    author: Option<String>,
    category: Option<Vec<String>>,
}

impl MangaDto {
    fn catalog_item(self) -> CatalogItem {
        let key = format!("{}:{}", self.id, self.name_en);
        CatalogItem {
            key: key.clone(),
            title: self.name,
            cover: Some(absolute_url(&self.photo)),
            authors: self
                .author
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            description: self.description.filter(|value| !value.is_empty()),
            tags: self.category.unwrap_or_default(),
            status: self
                .status_code
                .as_deref()
                .map(status_from_code)
                .unwrap_or(ItemStatus::Unknown),
            url: Some(format!(
                "{BASE_URL}/truyen/{}",
                key.split(':').nth(1).unwrap_or("")
            )),
            language: Some("vi".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

fn status_from_code(code: &str) -> ItemStatus {
    match code {
        "PRG" => ItemStatus::Ongoing,
        "END" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

#[derive(Deserialize)]
struct ChapterListDto {
    chapters: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    #[serde(rename = "comicId")]
    comic_id: String,
    #[serde(rename = "numberChapter")]
    number_chapter: String,
    #[serde(rename = "updateTime")]
    update_time: i64,
}

#[derive(Deserialize)]
struct ImageListDto {
    data: Option<Vec<String>>,
}

const LIST_FIXTURE: &str = r#"{"result":{"next":false,"data":[{"id":"1","name":"Sample","description":"Summary","statusCode":"PRG","photo":"/cover.jpg","nameEn":"sample","author":"Author","category":["Action"]}]}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="v-card-title">Sample</div><img class="image" src="/cover.jpg"><a class="v-chip-link">Action</a><div class="mb-1">Tác giả: <span>Author</span></div><div class="mb-1">Trạng thái: <span>Đang thực hiện</span></div><div class="v-card-text">Summary</div><script>const comic = {id: "1", nameEn: `sample`}</script>"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"result":{"chapters":[{"comicId":"1","numberChapter":"1","updateTime":1704067200000}]}}"#;
const PAGES_FIXTURE: &str = r#"{"result":{"data":["/image/page1.jpg"]}}"#;

export_manga_source!(SOURCE);
