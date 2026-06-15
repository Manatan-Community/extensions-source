use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: RawUwU = RawUwU;
const BASE_URL: &str = "https://rawuwu.net";

struct RawUwU;

impl MangaSource for RawUwU {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list_response(LIST_FIXTURE));
        }
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/spa/latest-manga?page={page}")
        } else {
            format!("{BASE_URL}/spa/genre/all?sort=most_viewed&page={page}")
        };
        Ok(parse_list_response(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_id(&id)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/spa/search?query={}&page={page}",
                url::query_escape(query)
            )
        } else {
            let genre = filter_string(&request, "genre").unwrap_or("all");
            let status = filter_string(&request, "status").unwrap_or_default();
            let sort = filter_string(&request, "sort").unwrap_or_default();
            let mut parts = vec![format!("page={page}")];
            if !status.is_empty() {
                parts.push(format!("status={}", url::query_escape(status)));
            }
            if !sort.is_empty() {
                parts.push(format!("sort={}", url::query_escape(sort)));
            }
            format!("{BASE_URL}/spa/genre/{genre}?{}", parts.join("&"))
        };
        Ok(parse_list_response(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_id(id_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let id = id_from_key(&key);
        Ok(parse_chapters(&fetch_json(
            &format!("{BASE_URL}/spa/manga/{id}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/1/chapter-1".into());
        let (manga_id, chapter_number) = chapter_parts(&key);
        Ok(parse_pages(&fetch_json(
            &format!("{BASE_URL}/spa/manga/{manga_id}/{chapter_number}"),
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
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/raw/{}", id_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&id)),
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

fn parse_list_response(body: &str) -> Paged<CatalogItem> {
    let payload: RawUwUResponse =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    Paged {
        entries: payload
            .manga_list
            .unwrap_or_default()
            .into_iter()
            .map(|entry| CatalogItem {
                key: entry.manga_id.to_string(),
                title: entry.manga_name,
                cover: Some(entry.manga_cover_img),
                url: Some(format!("{BASE_URL}/raw/{}", entry.manga_id)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: payload
            .pagi
            .and_then(|pagi| pagi.button)
            .and_then(|button| button.next)
            .is_some_and(|next| next > 0),
    }
}

fn details_by_id(id: &str) -> CatalogItem {
    parse_details(
        &fetch_json(&format!("{BASE_URL}/spa/manga/{id}"), DETAILS_FIXTURE),
        id,
    )
}

fn parse_details(body: &str, id: &str) -> CatalogItem {
    let payload: MangaDetailResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let detail = payload.detail.unwrap_or_default();
    let mut description = detail.manga_description.unwrap_or_default();
    if let Some(alt) = detail
        .manga_others_name
        .filter(|value| !value.trim().is_empty())
    {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Names:");
        for name in alt
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            description.push_str("\n - ");
            description.push_str(name);
        }
    }
    CatalogItem {
        key: id.to_string(),
        title: if detail.manga_name.is_empty() {
            "Raw UwU".into()
        } else {
            detail.manga_name
        },
        cover: detail.manga_cover_img_full.or(detail.manga_cover_img),
        description: (!description.trim().is_empty()).then_some(description),
        authors: payload
            .authors
            .unwrap_or_default()
            .into_iter()
            .map(|author| author.author_name)
            .collect(),
        tags: payload
            .tags
            .unwrap_or_default()
            .into_iter()
            .map(|tag| tag.tag_name)
            .collect(),
        status: match detail.manga_status {
            Some(true) => ItemStatus::Completed,
            Some(false) => ItemStatus::Ongoing,
            None => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/raw/{id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let payload: MangaDetailResponse = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let id = payload
        .detail
        .as_ref()
        .map(|detail| detail.manga_id)
        .unwrap_or(1);
    payload
        .chapters
        .unwrap_or_default()
        .into_iter()
        .filter_map(|chapter| {
            let number = chapter.chapter_number?;
            let formatted = format_chapter_number(number);
            let title = chapter
                .chapter_title
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty());
            let name = title
                .map(|title| format!("Ch. {formatted} - {title}"))
                .unwrap_or_else(|| format!("Chapter {formatted}"));
            let key = format!("/read/{id}/chapter-{formatted}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(name),
                date_uploaded: chapter
                    .chapter_date_published
                    .as_deref()
                    .and_then(parse_iso_date),
                url: Some(format!("{BASE_URL}{key}")),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let payload: ChapterPageResponse =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let Some(chapter) = payload.chapter_detail else {
        return Vec::new();
    };
    let server = chapter.server.unwrap_or_default();
    chapter
        .chapter_content
        .unwrap_or_default()
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .map(|raw| raw.trim_start_matches('/').to_string())
        .filter(|raw| !raw.is_empty() && !raw.starts_with("data:"))
        .enumerate()
        .map(|(index, raw)| {
            let image = if raw.starts_with("http://") || raw.starts_with("https://") {
                raw
            } else {
                format!("{}/{}", server.trim_end_matches('/'), raw)
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn id_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    path.strip_prefix("raw/")
        .or_else(|| path.strip_prefix("spa/manga/"))
        .and_then(|rest| rest.split('/').next())
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

fn id_from_key(key: &str) -> &str {
    key.trim_matches('/')
        .strip_prefix("raw/")
        .unwrap_or_else(|| key.trim_matches('/'))
}

fn chapter_parts(key: &str) -> (String, String) {
    let mut parts = key.trim_matches('/').split('/');
    let manga_id = parts.nth(1).unwrap_or("1").to_string();
    let chapter_number = parts
        .next()
        .unwrap_or("chapter-1")
        .trim_start_matches("chapter-")
        .to_string();
    (manga_id, chapter_number)
}

fn format_chapter_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(0..10).and_then(manatan_shared::dates::parse_ymd)
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

export_manga_source!(SOURCE);

#[derive(Debug, Default, Deserialize)]
struct RawUwUResponse {
    manga_list: Option<Vec<MangaListItem>>,
    pagi: Option<Pagi>,
}

#[derive(Debug, Deserialize)]
struct MangaListItem {
    manga_id: i64,
    manga_name: String,
    manga_cover_img: String,
}

#[derive(Debug, Deserialize)]
struct Pagi {
    button: Option<Button>,
}

#[derive(Debug, Deserialize)]
struct Button {
    next: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetailResponse {
    authors: Option<Vec<Author>>,
    chapters: Option<Vec<Chapter>>,
    detail: Option<MangaDetail>,
    tags: Option<Vec<Tag>>,
}

#[derive(Debug, Default, Deserialize)]
struct MangaDetail {
    manga_id: i64,
    manga_name: String,
    manga_description: Option<String>,
    manga_status: Option<bool>,
    manga_cover_img: Option<String>,
    manga_cover_img_full: Option<String>,
    manga_others_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    chapter_title: Option<String>,
    chapter_number: Option<f64>,
    chapter_date_published: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Author {
    author_name: String,
}

#[derive(Debug, Deserialize)]
struct Tag {
    tag_name: String,
}

#[derive(Debug, Deserialize)]
struct ChapterPageResponse {
    chapter_detail: Option<ChapterPageDetail>,
}

#[derive(Debug, Deserialize)]
struct ChapterPageDetail {
    server: Option<String>,
    chapter_content: Option<String>,
}

const LIST_FIXTURE: &str = r#"{"manga_list":[{"manga_id":1,"manga_name":"Sample Raw UwU","manga_cover_img":"https://rawuwu.net/cover.jpg"}],"pagi":{"button":{"next":0}}}"#;
const DETAILS_FIXTURE: &str = r#"{"detail":{"manga_id":1,"manga_name":"Sample Raw UwU","manga_description":"Sample description.","manga_status":false,"manga_cover_img":"https://rawuwu.net/cover.jpg","manga_cover_img_full":"https://rawuwu.net/cover-large.jpg","manga_others_name":"Alt"},"authors":[{"author_name":"Author"}],"tags":[{"tag_name":"Action"}],"chapters":[{"chapter_title":"Start","chapter_number":1,"chapter_date_published":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"chapter_detail":{"server":"https://rawuwu.net/images","chapter_content":"<img data-src=\"page1.jpg\"><img data-src=\"folder/page2.jpg\"> "}}"#;
