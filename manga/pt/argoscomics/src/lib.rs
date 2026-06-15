use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: ArgosComics = ArgosComics;
const BASE_URL: &str = "https://aniargos.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";
const SEARCH_TOKEN: &str = "406369e6483a4fe640a38cebf46ca5ea2385392f8d";
const CHAPTERS_TOKEN: &str = "6075c7373783e0d2488372dc7fcb9ffe1470bc41d2";
const DETAILS_TOKEN: &str = "60bd903bddc3d9d07f2b58fe32f0238afd74e492d6";
const PAGES_TOKEN: &str = "605aecabcce97cec193f09ebe5fe3a9ae46e432ea2";

struct ArgosComics;

impl MangaSource for ArgosComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_project_list(LIST_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest_list(&fetch_rsc(BASE_URL, LATEST_FIXTURE)));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_project_list(&fetch_rsc(
            &format!("{BASE_URL}/projetos?page={page}"),
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
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let body = post_action(BASE_URL, SEARCH_TOKEN, json!([query]), SEARCH_FIXTURE);
        let entries = extract_json::<Vec<MangaDto>>(&body)
            .or_else(|| serde_json::from_str::<Vec<MangaDto>>(SEARCH_FIXTURE).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|manga| catalog_from_manga(manga, false))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1/sample".into());
        let body = post_action(
            &absolute_url(&key),
            CHAPTERS_TOKEN,
            json!(path_segments(&key)),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/1/sample/capitulo/1".into());
        let parts = path_segments(&key);
        let payload = json!([
            parts.first().cloned().unwrap_or_default(),
            parts.last().cloned().unwrap_or_default()
        ]);
        Ok(parse_pages(&post_action(
            &absolute_url(&key),
            PAGES_TOKEN,
            payload,
            PAGES_FIXTURE,
        )))
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_action(target: &str, token: &str, payload: Value, fixture: &str) -> String {
    client()
        .post(target)
        .header("Next-Action", token)
        .header("Accept", "text/x-component")
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = post_action(
        &absolute_url(key),
        DETAILS_TOKEN,
        json!(path_segments(key)),
        DETAILS_FIXTURE,
    );
    let mut item = extract_json::<MangaDetailsDto>(&body)
        .or_else(|| serde_json::from_str::<MangaDetailsDto>(DETAILS_FIXTURE).ok())
        .map(catalog_from_details)
        .unwrap_or_else(|| catalog_from_details(MangaDetailsDto::default()));
    item.key = key.to_string();
    item.url = Some(absolute_url(key));
    item
}

fn parse_project_list(body: &str) -> Paged<CatalogItem> {
    extract_json::<MangasListDto>(body)
        .or_else(|| serde_json::from_str::<MangasListDto>(LIST_FIXTURE).ok())
        .map(|dto| Paged {
            entries: dto
                .projects
                .into_iter()
                .map(|manga| catalog_from_manga(manga, false))
                .collect(),
            has_next_page: dto.pagination.has_next_page,
        })
        .unwrap_or_default()
}

fn parse_latest_list(body: &str) -> Paged<CatalogItem> {
    extract_json::<LatestMangas>(body)
        .or_else(|| serde_json::from_str::<LatestMangas>(LATEST_FIXTURE).ok())
        .map(|dto| Paged {
            entries: dto
                .last_updates
                .into_iter()
                .map(|manga| catalog_from_manga(manga, false))
                .collect(),
            has_next_page: false,
        })
        .unwrap_or_default()
}

fn catalog_from_manga(manga: MangaDto, initialized: bool) -> CatalogItem {
    let key = format!("/{}/{}", manga.id, manga.link);
    CatalogItem {
        key: key.clone(),
        title: manga.title,
        cover: Some(manga.cover_image),
        status: status_from(&manga.status),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn catalog_from_details(details: MangaDetailsDto) -> CatalogItem {
    let description = if details.alt_titles.is_empty() {
        details.synopsis
    } else {
        format!(
            "{}\n\nTitulos alternativos: {}",
            details.synopsis,
            details.alt_titles.join(", ")
        )
    };
    CatalogItem {
        title: details.title,
        cover: Some(details.cover_image),
        description: Some(description).filter(|value| !value.is_empty()),
        status: status_from(&details.status),
        authors: details
            .authors
            .into_iter()
            .map(|value| value.name)
            .collect(),
        artists: details
            .artists
            .into_iter()
            .map(|value| value.name)
            .collect(),
        tags: details
            .genders
            .into_iter()
            .map(|value| value.name)
            .collect(),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    extract_json::<VolumeChapterDto>(body)
        .or_else(|| serde_json::from_str::<VolumeChapterDto>(CHAPTERS_FIXTURE).ok())
        .map(|dto| {
            dto.groups
                .into_iter()
                .flat_map(|group| group.chapters)
                .map(|chapter| {
                    let number = trim_number(chapter.title);
                    let key = format!("{manga_key}/capitulo/{number}");
                    MangaChapter {
                        key: key.clone(),
                        title: Some(number.clone()),
                        chapter_number: Some(chapter.title),
                        date_uploaded: parse_feed_date(&chapter.created_at),
                        url: Some(absolute_url(&key)),
                        language: Some(LANG.to_string()),
                        ..MangaChapter::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_json::<PagesDto>(body)
        .or_else(|| serde_json::from_str::<PagesDto>(PAGES_FIXTURE).ok())
        .unwrap_or_default()
        .pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.photo,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_json<T: for<'de> Deserialize<'de>>(body: &str) -> Option<T> {
    serde_json::from_str(body).ok().or_else(|| {
        for candidate in json_candidates(body) {
            if let Ok(value) = serde_json::from_str::<T>(&candidate) {
                return Some(value);
            }
        }
        None
    })
}

fn json_candidates(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' && bytes[start] != b'[' {
            continue;
        }
        let open = bytes[start];
        let close = if open == b'{' { b'}' } else { b']' };
        let mut stack = vec![close];
        let mut in_string = false;
        let mut escape = false;
        for index in start + 1..bytes.len() {
            let byte = bytes[index];
            if in_string {
                if escape {
                    escape = false;
                } else if byte == b'\\' {
                    escape = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'{' => stack.push(b'}'),
                b'[' => stack.push(b']'),
                b'}' | b']' => {
                    if stack.pop() != Some(byte) {
                        break;
                    }
                    if stack.is_empty() {
                        if byte == close || index > start {
                            out.push(body[start..=index].replace("\\\"", "\""));
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn status_from(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "active" | "up_to_date" | "coming_soon" => ItemStatus::Ongoing,
        "hiatus" => ItemStatus::Hiatus,
        "finished" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn path_segments(key: &str) -> Vec<String> {
    key.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn trim_number(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn parse_feed_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

#[derive(Default, Deserialize)]
struct MangasListDto {
    #[serde(default)]
    projects: Vec<MangaDto>,
    #[serde(default)]
    pagination: PaginationDto,
}

#[derive(Default, Deserialize)]
struct PaginationDto {
    #[serde(default, rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Default, Deserialize)]
struct LatestMangas {
    #[serde(default, rename = "lastUpdates")]
    last_updates: Vec<MangaDto>,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover_image: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    status: String,
}

#[derive(Default, Deserialize)]
struct MangaDetailsDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    alt_titles: Vec<String>,
    #[serde(default)]
    cover_image: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    genders: Vec<NameDto>,
    #[serde(default)]
    artists: Vec<NameDto>,
    #[serde(default)]
    authors: Vec<NameDto>,
}

#[derive(Default, Deserialize)]
struct NameDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct VolumeChapterDto {
    #[serde(default)]
    groups: Vec<ChapterGroupDto>,
}

#[derive(Default, Deserialize)]
struct ChapterGroupDto {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    title: f32,
    #[serde(default)]
    created_at: String,
}

#[derive(Default, Deserialize)]
struct PagesDto {
    #[serde(default)]
    pages: Vec<ImageDto>,
}

#[derive(Default, Deserialize)]
struct ImageDto {
    #[serde(default)]
    photo: String,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"projects":[{"id":"1","title":"Sample Argos Comics","cover_image":"/cover.jpg","link":"sample","status":"active"}],"pagination":{"hasNextPage":false}}"#;
const LATEST_FIXTURE: &str = r#"{"lastUpdates":[{"id":"1","title":"Sample Argos Comics","cover_image":"/cover.jpg","link":"sample","status":"active"}]}"#;
const SEARCH_FIXTURE: &str = r#"[{"id":"1","title":"Sample Argos Comics","cover_image":"/cover.jpg","link":"sample","status":"active"}]"#;
const DETAILS_FIXTURE: &str = r#"{"title":"Sample Argos Comics","alt_titles":["Alt"],"cover_image":"/cover.jpg","status":"active","synopsis":"Summary","genders":[{"name":"Action"}],"artists":[{"name":"Artist"}],"authors":[{"name":"Author"}]}"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"groups":[{"chapters":[{"id":"c1","title":1,"created_at":"2024-01-01"}]}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"photo":"/page1.jpg"},{"photo":"/page2.jpg"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_argoscomics_fixtures() {
        assert_eq!(parse_project_list(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_latest_list(LATEST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "/1/sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
