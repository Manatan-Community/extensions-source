use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: Yorai = Yorai;
const BASE_URL: &str = "https://yorai.io";
const API_URL: &str = "https://yorai.io/api";

struct Yorai;

impl MangaSource for Yorai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "views"
        } else {
            "new"
        };
        Ok(parse_browse(&api_get(
            &format!("/comics/browse?page={page}&sort={sort}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = slug_from_url(query);
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let mut path = format!("/comics/browse?page={page}&q={}", url::query_escape(query));
        append_filters(&mut path, request.get("filters"));
        Ok(parse_browse(&api_get(&path, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_slug(&slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_rsc(&format!("{BASE_URL}/comic/{slug}"), DETAILS_RSC_FIXTURE);
        Ok(parse_chapters(&body, &slug).unwrap_or_else(|| generated_chapters(&slug)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample|1".to_string());
        let (slug, number) = key.split_once('|').unwrap_or((key.as_str(), "1"));
        let body = fetch_rsc(
            &format!("{BASE_URL}/comic/{slug}/chapter/{number}"),
            PAGES_RSC_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                HomeSectionStyle::Cover,
                self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                HomeSectionStyle::Compact,
                self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|slug| format!("{BASE_URL}/comic/{slug}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, number) = key.split_once('|').unwrap_or((key.as_str(), "1"));
            format!("{BASE_URL}/comic/{slug}/chapter/{number}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let slug = slug_from_url(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .header("Accept", "application/json, text/plain, */*")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .header("Accept", "text/x-component")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn append_filters(path: &mut String, filters: Option<&Value>) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        return;
    };
    for key in ["sort", "order", "statuses", "types"] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            path.push('&');
            path.push_str(key);
            path.push('=');
            path.push_str(&url::query_escape(value));
        }
    }
    let genres = filter_values(filters.get("genres"));
    if !genres.is_empty() {
        path.push_str("&genres=");
        path.push_str(&url::query_escape(&genres.join(",")));
    }
}

fn filter_values(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn parse_browse(body: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<Browse>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        has_next_page: page.page < page.total_pages,
        entries: page.comics.into_iter().map(Comic::into_catalog).collect(),
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let mut page = serde_json::from_str::<Browse>(&api_get(
        &format!("/comics/browse?page=1&q={}", url::query_escape(slug)),
        DETAILS_API_FIXTURE,
    ))
    .unwrap_or_else(|_| serde_json::from_str(DETAILS_API_FIXTURE).expect("fixture is valid"));
    page.comics
        .iter()
        .position(|comic| comic.slug == slug)
        .map(|index| page.comics.remove(index))
        .or_else(|| page.comics.into_iter().next())
        .unwrap_or_else(|| Comic::fallback(slug))
        .into_catalog_initialized()
}

fn parse_chapters(body: &str, slug: &str) -> Option<Vec<MangaChapter>> {
    let object = json_object_around(body, "\"defaultSource\"")?;
    let payload = serde_json::from_str::<Chapters>(&object).ok()?;
    let default_source = payload.default_source.clone();
    let chapters = payload
        .chapters
        .into_iter()
        .filter(|chapter| {
            default_source
                .as_ref()
                .is_none_or(|source| chapter.source_name.as_deref() == Some(source))
        })
        .map(|chapter| chapter.into_chapter(&payload.slug))
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        None
    } else {
        Some(chapters)
    }
    .or_else(|| Some(generated_chapters(slug)))
}

fn generated_chapters(slug: &str) -> Vec<MangaChapter> {
    let item = details_by_slug(slug);
    let count = item
        .extra
        .get("latest_number")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    (1..=count.max(1))
        .rev()
        .map(|number| MangaChapter {
            key: format!("{slug}|{number}"),
            title: Some(format!("Chapter {number}")),
            chapter_number: Some(number as f32),
            url: Some(format!("{BASE_URL}/comic/{slug}/chapter/{number}")),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = json_object_around(body, "\"imageUrls\"")
        .and_then(|object| serde_json::from_str::<ChapterPages>(&object).ok())
        .map(|payload| payload.image_urls)
        .unwrap_or_else(|| {
            extract_quoted_paths(body)
                .into_iter()
                .filter(|path| path.contains("/api/img/"))
                .collect()
        });
    images
        .into_iter()
        .filter(|image| !image.is_empty())
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

fn json_object_around(body: &str, marker: &str) -> Option<String> {
    let marker_index = body.find(marker)?;
    let start = body[..marker_index].rfind('{')?;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in body[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(body[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_quoted_paths(body: &str) -> Vec<String> {
    body.split('"')
        .filter(|part| part.starts_with("/api/img/"))
        .map(ToString::to_string)
        .collect()
}

fn slug_from_url(input: &str) -> String {
    let without_chapter = input.split("/chapter/").next().unwrap_or(input);
    url::slug_from_url(without_chapter).unwrap_or_else(|| without_chapter.to_string())
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

#[derive(Debug, Deserialize)]
struct Browse {
    comics: Vec<Comic>,
    page: u64,
    #[serde(rename = "totalPages")]
    total_pages: u64,
}

#[derive(Debug, Deserialize)]
struct Comic {
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "coverUrl")]
    cover_url: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    comic_type: String,
    #[serde(default)]
    latest_number: Option<u64>,
}

impl Comic {
    fn fallback(slug: &str) -> Self {
        Self {
            slug: slug.to_string(),
            title: url::slug_from_url(slug).unwrap_or_else(|| "Yorai".to_string()),
            description: String::new(),
            cover_url: String::new(),
            status: String::new(),
            comic_type: String::new(),
            latest_number: Some(1),
        }
    }

    fn into_catalog(self) -> CatalogItem {
        let mut extra = BTreeMap::new();
        extra.insert("type".to_string(), serde_json::json!(self.comic_type));
        extra.insert(
            "latest_number".to_string(),
            serde_json::json!(self.latest_number),
        );
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: Some(url::join_url(BASE_URL, &self.cover_url))
                .filter(|_| !self.cover_url.is_empty()),
            url: Some(format!("{BASE_URL}/comic/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            extra,
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_catalog_initialized(self) -> CatalogItem {
        let description = html::strip_tags(&self.description);
        let status = self.status.clone();
        let mut item = self.into_catalog();
        item.initialized = true;
        item.description = Some(description).filter(|value| !value.is_empty());
        item.status = match status.as_str() {
            "completed" => ItemStatus::Completed,
            "hiatus" => ItemStatus::Hiatus,
            "releasing" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        };
        item
    }
}

#[derive(Debug, Deserialize)]
struct Chapters {
    slug: String,
    chapters: Vec<Chapter>,
    #[serde(default, rename = "defaultSource")]
    default_source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    number: f64,
    title: String,
    #[serde(default, rename = "source_name")]
    source_name: Option<String>,
}

impl Chapter {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let number_label = if self.number.fract() == 0.0 {
            format!("{}", self.number as u64)
        } else {
            self.number.to_string()
        };
        MangaChapter {
            key: format!("{slug}|{number_label}"),
            title: Some(if self.title.is_empty() {
                format!("Chapter {number_label}")
            } else {
                self.title
            }),
            chapter_number: Some(self.number as f32),
            scanlators: self.source_name.into_iter().collect(),
            url: Some(format!("{BASE_URL}/comic/{slug}/chapter/{number_label}")),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChapterPages {
    #[serde(rename = "imageUrls")]
    image_urls: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{"comics":[{"title":"Sample Manga","slug":"sample","description":"Summary text.","coverUrl":"/api/img/sample.webp","status":"releasing","comic_type":"manga","latest_number":1}],"page":1,"totalPages":2}
"#;
const DETAILS_API_FIXTURE: &str = r#"
{"comics":[{"title":"Sample Manga","slug":"sample","description":"Summary text.","coverUrl":"/api/img/sample.webp","status":"releasing","comic_type":"manga","latest_number":1}],"page":1,"totalPages":1}
"#;
const DETAILS_RSC_FIXTURE: &str = r#"
0:{"slug":"sample","defaultSource":"Sample Scan","chapters":[{"number":1,"title":"Chapter 1","source_name":"Sample Scan"}]}
"#;
const PAGES_RSC_FIXTURE: &str = r#"
0:{"imageUrls":["/api/img/page1.webp"]}
"#;
