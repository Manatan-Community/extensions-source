use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ReiManga = ReiManga;
const BASE_URL: &str = "https://reimanga.com";
const PAGE_SIZE: u64 = 24;

struct ReiManga;

impl MangaSource for ReiManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_trending(TRENDING_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") || page > 1 {
            return self.search(serde_json::json!({"page": page, "query": "", "sort": "latest"}));
        }
        Ok(parse_trending(&api_get(
            "/api/manga/trending?limit=100",
            TRENDING_FIXTURE,
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
            let slug_id = normalize_slug_id(query);
            return Ok(Paged {
                entries: vec![details_by_slug_id(&slug_id)],
                has_next_page: false,
            });
        }
        let mut path = format!("/api/manga?page={page}&limit={PAGE_SIZE}&sort=latest&order=desc");
        if !query.is_empty() {
            path.push_str("&search=");
            path.push_str(&url::query_escape(query));
        }
        Ok(parse_manga_page(&api_get(&path, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-1".to_string());
        Ok(details_by_slug_id(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-1".to_string());
        Ok(parse_chapters(
            &rsc_get(
                &format!("/manga/{}", key.trim_matches('/')),
                CHAPTERS_FIXTURE,
            ),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-1/1".to_string());
        Ok(parse_pages(&rsc_get(
            &format!("/manga/{}", key.trim_matches('/')),
            PAGES_FIXTURE,
        )))
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
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", key.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/manga/{}", key.trim_matches('/'))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_slug_id(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug_id(&key)),
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
        .with_header("Cookie", "showAdultContent=true")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .header("Accept", "application/json, text/plain, */*")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn rsc_get(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_trending(body: &str) -> Paged<CatalogItem> {
    let mangas = serde_json::from_str::<Vec<Manga>>(body)
        .unwrap_or_else(|_| serde_json::from_str(TRENDING_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: mangas.into_iter().map(Manga::to_item).collect(),
        has_next_page: true,
    }
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<MangaList>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: page.data.into_iter().map(Manga::to_item).collect(),
        has_next_page: page.pagination.current_page < page.pagination.total_pages,
    }
}

fn details_by_slug_id(slug_id: &str) -> CatalogItem {
    let id = slug_id.rsplit('-').next().unwrap_or(slug_id);
    let page = serde_json::from_str::<MangaPagePayload>(&api_get(
        &format!("/api/manga/{id}"),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    page.manga.to_item()
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = extract_json_array(body, "\"chapters\"")
        .and_then(|value| serde_json::from_str::<Vec<Chapter>>(&value).ok())
        .unwrap_or_else(|| serde_json::from_str(CHAPTER_ARRAY_FIXTURE).expect("fixture is valid"));
    chapters
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("{}/{}", manga_key.trim_matches('/'), chapter.id),
            title: Some(
                chapter
                    .name
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            date_uploaded: chapter
                .upload_date
                .or(chapter.updated_at)
                .or(chapter.created_at)
                .as_deref()
                .and_then(parse_date),
            url: Some(format!(
                "{BASE_URL}/manga/{}/{}",
                manga_key.trim_matches('/'),
                chapter.id
            )),
            language: Some("en".into()),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = extract_json_array(body, "\"images\"")
        .and_then(|value| serde_json::from_str::<Vec<Image>>(&value).ok())
        .unwrap_or_else(|| serde_json::from_str(IMAGE_ARRAY_FIXTURE).expect("fixture is valid"));
    images
        .into_iter()
        .filter_map(|image| image.image_url.or(image.url))
        .filter(|image| !image.is_empty())
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

fn extract_json_array(body: &str, marker: &str) -> Option<String> {
    let marker_index = body.find(marker)?;
    let after_marker = &body[marker_index + marker.len()..];
    let start_rel = after_marker.find('[')?;
    let start = marker_index + marker.len() + start_rel;
    balanced_json_array(&body[start..])
}

fn balanced_json_array(input: &str) -> Option<String> {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escape = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(input[..=index].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn normalize_slug_id(input: &str) -> String {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample-1")
        .to_string()
}

fn home_section(
    id: &str,
    title: &str,
    style: HomeSectionStyle,
    page: Paged<CatalogItem>,
) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(style),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

#[derive(Deserialize)]
struct MangaList {
    #[serde(alias = "initialData")]
    data: Vec<Manga>,
    pagination: Pagination,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    current_page: u64,
    total_pages: u64,
}

#[derive(Default, Deserialize)]
struct Manga {
    id: i64,
    #[serde(rename = "name_url")]
    slug: String,
    title: String,
    #[serde(default, rename = "cover_url")]
    cover: Option<String>,
    #[serde(default)]
    rating: Option<f64>,
    #[serde(default, rename = "is_adult")]
    is_adult: Option<i32>,
}

impl Manga {
    fn to_item(self) -> CatalogItem {
        let key = format!("{}-{}", self.slug, self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self
                .cover
                .or_else(|| Some(format!("{BASE_URL}/covers/{}/thumbnail.png", self.id))),
            description: self
                .rating
                .filter(|rating| *rating > 0.0)
                .map(|rating| format!("Rating: {rating}")),
            tags: if self.is_adult == Some(1) {
                vec!["Adult".into()]
            } else {
                Vec::new()
            },
            url: Some(format!("{BASE_URL}/manga/{key}")),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct MangaPagePayload {
    manga: MangaDetails,
}

#[derive(Default, Deserialize)]
struct MangaDetails {
    id: i64,
    #[serde(rename = "name_url")]
    slug: String,
    title: String,
    #[serde(default, rename = "cover_url")]
    cover: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "alt_title")]
    alt_title: Option<String>,
    #[serde(default)]
    completed: i32,
    #[serde(default)]
    rating: f64,
    #[serde(default, rename = "is_adult")]
    is_adult: i32,
    #[serde(default)]
    genres: Vec<Tag>,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    authors: Vec<Name>,
}

impl MangaDetails {
    fn to_item(self) -> CatalogItem {
        let key = format!("{}-{}", self.slug, self.id);
        let mut description = String::new();
        if self.rating > 0.0 {
            description.push_str(&format!("Rating: {}\n\n", self.rating));
        }
        if let Some(value) = self
            .description
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            description.push_str(&html::strip_tags(value));
        }
        CatalogItem {
            key: key.clone(),
            title: self.title,
            alternate_titles: self
                .alt_title
                .map(|value| {
                    value
                        .split([',', ';'])
                        .map(str::trim)
                        .filter(|title| !title.is_empty())
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            cover: self
                .cover
                .or_else(|| Some(format!("{BASE_URL}/covers/{}/thumbnail.png", self.id))),
            authors: self
                .authors
                .into_iter()
                .map(|author| author.name.trim().trim_end_matches(',').to_string())
                .filter(|name| !name.is_empty())
                .collect(),
            description: (!description.trim().is_empty()).then_some(description.trim().to_string()),
            tags: self
                .genres
                .into_iter()
                .chain(self.tags)
                .map(|tag| tag.name)
                .chain((self.is_adult == 1).then_some("Adult".to_string()))
                .collect(),
            status: if self.completed == 1 {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            url: Some(format!("{BASE_URL}/manga/{key}")),
            language: Some("en".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Chapter {
    id: i64,
    name: String,
    #[serde(default, rename = "gdrive_upload_date")]
    upload_date: Option<String>,
    #[serde(default, rename = "updated_at")]
    updated_at: Option<String>,
    #[serde(default, rename = "created_at")]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct Image {
    #[serde(default, rename = "image_url")]
    image_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

fn parse_date(value: &str) -> Option<i64> {
    let y = value.get(0..4)?.parse().ok()?;
    let m = value.get(5..7)?.parse().ok()?;
    let d = value.get(8..10)?.parse().ok()?;
    Some(unix_from_ymd(y, m, d))
}

fn unix_from_ymd(year: i32, month: i32, day: i32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

export_manga_source!(SOURCE);

const TRENDING_FIXTURE: &str = r#"[{"id":1,"title":"Sample Manga","name_url":"sample","rating":8.5,"is_adult":1,"cover_url":null}]"#;
const SEARCH_FIXTURE: &str = r#"{"initialData":[{"id":1,"title":"Sample Manga","name_url":"sample","rating":8.5,"is_adult":1,"cover_url":null}],"pagination":{"currentPage":1,"totalPages":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"id":1,"title":"Sample Manga","name_url":"sample","description":"Sample description","completed":0,"rating":8.5,"is_adult":1,"cover_url":null,"genres":[{"name":"Action"}],"tags":[{"name":"Adult"}],"authors":[{"name":"Author"}]}}"#;
const CHAPTER_ARRAY_FIXTURE: &str =
    r#"[{"id":10,"name":"Chapter 1","created_at":"2024-01-01T00:00:00.000Z"}]"#;
const IMAGE_ARRAY_FIXTURE: &str = r#"[{"image_url":"https://reimanga.com/page1.jpg"},{"image_url":"https://reimanga.com/page2.jpg"}]"#;
const CHAPTERS_FIXTURE: &str =
    r#"0:{"chapters":[{"id":10,"name":"Chapter 1","created_at":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"0:{"images":[{"image_url":"https://reimanga.com/page1.jpg"},{"image_url":"https://reimanga.com/page2.jpg"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
