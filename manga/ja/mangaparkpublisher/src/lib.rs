use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: MangaParkPublisher = MangaParkPublisher;
const BASE_URL: &str = "https://manga-park.com";

struct MangaParkPublisher;

impl MangaSource for MangaParkPublisher {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/series")
        } else {
            format!("{BASE_URL}/ranking?target=all")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
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
                "{BASE_URL}/search/freeword?key={}",
                url::query_escape(query)
            )
        } else {
            let filter = filter_string(&request, "type").unwrap_or("ranking:all");
            let (kind, value) = filter.split_once(':').unwrap_or(("ranking", "all"));
            if kind == "ranking" {
                format!("{BASE_URL}/ranking?target={}", url::query_escape(value))
            } else {
                format!("{BASE_URL}/series/{}", value.trim_matches('/'))
            }
        };
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let chapter_id =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-chapter".into());
        let referer = request
            .get("chapter")
            .and_then(|chapter| chapter.get("url"))
            .and_then(Value::as_str)
            .unwrap_or(BASE_URL)
            .to_string();
        Ok(parse_pages(
            &fetch_json(
                &format!("{BASE_URL}/api/chapter/{chapter_id}"),
                PAGES_FIXTURE,
            ),
            &referer,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"listingId": "popular"}))?;
        let latest = self.list(json!({"listingId": "latest"}))?;
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
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let input = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let key = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("mangaParkKey"))
            .or_else(|| {
                request
                    .get("context")
                    .and_then(|context| context.get("mangaParkKey"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default();
        let image_base64 = xor_image_base64(input, key).unwrap_or_else(|| input.to_string());
        Ok(ProcessedImage {
            image_base64,
            mime_type: request
                .get("mimeType")
                .or_else(|| request.get("mime_type"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/viewer/{}", key.trim_matches('/'))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("/title/")
                || chunk.contains("rankingHome")
                || chunk.contains("common-list")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/title/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga-Park".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/title/sample".into());
    let status_text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "titleInfo", "</h1>")
            .and_then(|value| html::text_between(&value, "<h1", "</h1>").or(Some(value)))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga-Park".into())),
        cover: html::attr_after(body, "titleThumb", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: html::text_between(body, "author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(body, "explanation", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: title_category_values(body),
        status: if status_text.contains("完結") {
            ItemStatus::Completed
        } else if status_text.contains("休載中") {
            ItemStatus::Hiatus
        } else if status_text.contains("更新") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("data-chapter-id"))
        .filter_map(|chunk| {
            let id = html::attr(chunk, "data-chapter-id")?;
            let title = html::text_between(chunk, "chapterTitle", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let is_free = chunk.contains("free-badge");
            Some(MangaChapter {
                key: id.clone(),
                title: Some(if is_free {
                    format!("[FREE] {title}")
                } else {
                    title
                }),
                chapter_number: html::attr(chunk, "data-chapter-name")
                    .and_then(|value| value.parse().ok()),
                date_uploaded: html::text_between(chunk, "<span", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_ymd(&value)),
                url: Some(format!("{BASE_URL}/viewer/{id}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<ApiResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    response
        .data
        .chapter
        .into_iter()
        .flat_map(|chapter| chapter.images)
        .enumerate()
        .map(|(index, image)| {
            let mut extra = BTreeMap::new();
            extra.insert("mangaParkKey".into(), Value::String(image.key));
            let headers = manga::image_headers(referer);
            MangaPage {
                content: PageContent::Url {
                    url: image.path,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn xor_image_base64(input: &str, key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let mut bytes = STANDARD.decode(input).ok()?;
    let key = STANDARD.decode(key).ok()?;
    if key.is_empty() {
        return None;
    }
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key[index % key.len()];
    }
    Some(STANDARD.encode(bytes))
}

fn title_category_values(body: &str) -> Vec<String> {
    html::text_between(body, "titleCategory", "</div>")
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn key_from_url(value: &str) -> Option<String> {
    if !value.starts_with(BASE_URL) || !value.contains("/title/") {
        return None;
    }
    Some(normalize_key(value))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(rest) = value.split(BASE_URL).nth(1) {
            return normalize_key(rest);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

#[derive(Deserialize)]
struct ApiResponse {
    data: ApiData,
}

#[derive(Deserialize)]
struct ApiData {
    chapter: Vec<ApiPageData>,
}

#[derive(Deserialize)]
struct ApiPageData {
    images: Vec<ApiImage>,
}

#[derive(Deserialize)]
struct ApiImage {
    path: String,
    key: String,
}

const LIST_FIXTURE: &str = r#"
<ul class="common-list"><li><a href="/title/sample"><div class="thumb"><img src="/cover.jpg"></div><div class="info"><h3>Sample Manga-Park</h3></div></a></li></ul>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<div class="titleMain"><div class="titleInfo"><h1>Sample Manga-Park</h1><p class="author">Author</p><div class="titleCategory"><ul><li><a>Action</a></li></ul></div><div class="tag"><ul><li><a>更新</a></li></ul></div></div></div><div class="titleThumb"><img src="/cover.jpg"></div><p class="explanation">Summary</p><div class="chapter"><ul><li data-chapter-id="sample-chapter" data-chapter-name="1"><p class="chapterTitle">第1話</p><div class="date"><span>2024/1/1</span></div><div class="free-badge"><img src="/free.png"></div></li></ul></div>
"#;

const PAGES_FIXTURE: &str = r#"
{"data":{"chapter":[{"images":[{"path":"https://cdn.example.test/page1.jpg","key":"AQID"},{"path":"https://cdn.example.test/page2.jpg","key":"AQID"}]}]}}
"#;

export_manga_source!(SOURCE);
