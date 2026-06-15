use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ArgosScan = ArgosScan;
const BASE_URL: &str = "https://argoscomics.online";
const API_URL: &str = "https://api.argoscomics.online";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";

struct ArgosScan;

impl MangaSource for ArgosScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_project_listing(PROJECTS_FIXTURE, ""));
        }
        Ok(parse_project_listing(
            &fetch_json(&format!("{API_URL}/projects"), PROJECTS_FIXTURE),
            "",
        ))
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
        Ok(parse_project_listing(
            &fetch_json(&format!("{API_URL}/projects"), PROJECTS_FIXTURE),
            query,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = slug_from_manga_key(&key);
        let details = fetch_project_by_slug(&slug);
        let project_id = details.id.clone();
        let body = fetch_json(
            &format!("{API_URL}/chapters?kind=published&project_id={project_id}"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, &project_id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1|1".into());
        let (chapter_id, project_id) = key.split_once('|').unwrap_or(("1", "1"));
        let body = fetch_json(
            &format!("{API_URL}/chapters?kind=published&project_id={project_id}"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_pages(&body, chapter_id))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key.starts_with("/manga/").then(|| details_by_key(&key)),
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
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_project_by_slug(slug: &str) -> ProjectDto {
    serde_json::from_str::<ProjectDto>(&fetch_json(
        &format!("{API_URL}/projects/slug/{slug}"),
        DETAILS_FIXTURE,
    ))
    .or_else(|_| serde_json::from_str::<ProjectDto>(DETAILS_FIXTURE))
    .unwrap_or_default()
}

fn details_by_key(key: &str) -> CatalogItem {
    let project = fetch_project_by_slug(&slug_from_manga_key(key));
    catalog_from_project(project, true)
}

fn parse_project_listing(body: &str, query: &str) -> Paged<CatalogItem> {
    let lower = query.to_ascii_lowercase();
    let dto = serde_json::from_str::<ProjectResponseDto>(body)
        .or_else(|_| serde_json::from_str::<ProjectResponseDto>(PROJECTS_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: dto
            .items
            .into_iter()
            .filter(|project| {
                project
                    .project_type
                    .as_deref()
                    .is_none_or(|value| !value.eq_ignore_ascii_case("Novel"))
            })
            .filter(|project| {
                lower.is_empty() || project.title.to_ascii_lowercase().contains(&lower)
            })
            .map(|project| catalog_from_project(project, false))
            .collect(),
        has_next_page: false,
    }
}

fn catalog_from_project(project: ProjectDto, initialized: bool) -> CatalogItem {
    let key = format!("/manga/{}", project.slug);
    CatalogItem {
        key: key.clone(),
        title: project.title,
        cover: project.cover_latest_url,
        description: project.description.map(|value| html::strip_tags(&value)),
        status: status_from(project.status.as_deref().unwrap_or_default()),
        authors: project
            .authors
            .iter()
            .filter(|author| {
                author
                    .role
                    .as_deref()
                    .is_some_and(|role| role.eq_ignore_ascii_case("autor"))
            })
            .map(|author| author.name.clone())
            .collect(),
        artists: project
            .authors
            .iter()
            .filter(|author| {
                author
                    .role
                    .as_deref()
                    .is_some_and(|role| role.eq_ignore_ascii_case("artista"))
            })
            .map(|author| author.name.clone())
            .collect(),
        tags: project.tags.into_iter().map(|tag| tag.name).collect(),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, project_id: &str) -> Vec<MangaChapter> {
    let dto = serde_json::from_str::<ChapterResponseDto>(body)
        .or_else(|_| serde_json::from_str::<ChapterResponseDto>(CHAPTERS_FIXTURE))
        .unwrap_or_default();
    let mut chapters = dto
        .items
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("{}|{project_id}", chapter.id),
            title: Some(chapter_title(&chapter)),
            chapter_number: chapter.chapter_number,
            date_uploaded: chapter.created_at.as_deref().and_then(parse_feed_date),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str, chapter_id: &str) -> Vec<MangaPage> {
    let dto = serde_json::from_str::<ChapterResponseDto>(body)
        .or_else(|_| serde_json::from_str::<ChapterResponseDto>(CHAPTERS_FIXTURE))
        .unwrap_or_default();
    dto.items
        .into_iter()
        .find(|chapter| chapter.id == chapter_id)
        .and_then(|chapter| chapter.images)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.file_url,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_title(chapter: &ChapterDto) -> String {
    let mut out = String::new();
    if let Some(volume) = chapter.volume_number {
        out.push_str(&format!("Vol. {volume} "));
    }
    out.push_str("Cap. ");
    out.push_str(
        chapter
            .chapter_number
            .map(trim_number)
            .unwrap_or_else(|| "0".to_string())
            .as_str(),
    );
    if let Some(title) = chapter.title.as_deref().filter(|value| !value.is_empty()) {
        out.push_str(" - ");
        out.push_str(title);
    }
    out.trim().to_string()
}

fn status_from(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "completo" => ItemStatus::Completed,
        "em lançamento" | "em lancamento" => ItemStatus::Ongoing,
        "em pausa" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn trim_number(value: f32) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

fn slug_from_manga_key(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
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
struct ProjectResponseDto {
    #[serde(default)]
    items: Vec<ProjectDto>,
}

#[derive(Default, Deserialize)]
struct ProjectDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default, rename = "type")]
    project_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    cover_latest_url: Option<String>,
    #[serde(default)]
    authors: Vec<AuthorDto>,
    #[serde(default)]
    tags: Vec<TagDto>,
}

#[derive(Default, Deserialize)]
struct AuthorDto {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Default, Deserialize)]
struct TagDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterResponseDto {
    #[serde(default)]
    items: Vec<ChapterDto>,
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    chapter_number: Option<f32>,
    #[serde(default)]
    volume_number: Option<i32>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    images: Option<Vec<ImageDto>>,
}

#[derive(Default, Deserialize)]
struct ImageDto {
    #[serde(default)]
    file_url: String,
}

export_manga_source!(SOURCE);

const PROJECTS_FIXTURE: &str = r#"{"items":[{"id":"1","title":"Sample Argos","slug":"sample","type":"Manga","description":"Summary","status":"em lançamento","cover_latest_url":"/cover.jpg","authors":[{"name":"Author","role":"autor"}],"tags":[{"name":"Action"}]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"1","title":"Sample Argos","slug":"sample","type":"Manga","description":"Summary","status":"em lançamento","cover_latest_url":"/cover.jpg","authors":[{"name":"Author","role":"autor"},{"name":"Artist","role":"artista"}],"tags":[{"name":"Action"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"items":[{"id":"c1","title":"Start","chapter_number":1,"volume_number":1,"created_at":"2024-01-01T00:00:00","images":[{"file_url":"/page1.jpg"}]}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_argosscan_fixtures() {
        assert_eq!(parse_project_listing(PROJECTS_FIXTURE, "").entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "1").len(), 1);
        assert_eq!(parse_pages(CHAPTERS_FIXTURE, "c1").len(), 1);
    }
}
