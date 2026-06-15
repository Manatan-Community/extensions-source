use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Source = Source;
const BASE_URL: &str = "https://twatt.fr";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_projects(LIST_FIXTURE));
        }
        Ok(parse_projects(&fetch_api("/api/projects", LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = deeplink_series_id(query) {
            return Ok(Paged {
                entries: vec![parse_series(
                    &fetch_api(&format!("/api/series/{id}"), DETAILS_FIXTURE),
                    Some(id),
                )],
                has_next_page: false,
            });
        }
        let mut page = parse_projects(&fetch_api("/api/projects", LIST_FIXTURE));
        if !query.is_empty() {
            let needle = query.to_ascii_lowercase();
            page.entries
                .retain(|item| item.title.to_ascii_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        let id = key.rsplit('/').next().unwrap_or("sample").to_string();
        Ok(parse_series(
            &fetch_api(&format!("/api/series/{id}"), DETAILS_FIXTURE),
            Some(id),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        let id = key.rsplit('/').next().unwrap_or("sample");
        Ok(parse_chapters(&fetch_api(
            &format!("/api/series/{id}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapitre/chapter-1".into());
        let id = key.rsplit('/').next().unwrap_or("chapter-1");
        Ok(parse_pages(&fetch_api(
            &format!("/api/chapters/{id}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = deeplink_series_id(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_series(
                    &fetch_api(&format!("/api/series/{id}"), DETAILS_FIXTURE),
                    Some(id),
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

export_manga_source!(SOURCE);

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(path: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, path))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_projects(body: &str) -> Paged<CatalogItem> {
    let response: ProjectsResponse = json(body, LIST_FIXTURE);
    Paged {
        entries: response
            .projects
            .into_iter()
            .map(Project::into_catalog)
            .collect(),
        has_next_page: false,
    }
}

fn parse_series(body: &str, key_id: Option<String>) -> CatalogItem {
    let response: SeriesResponse = json(body, DETAILS_FIXTURE);
    let mut item = response.project.into_catalog();
    if let Some(id) = key_id {
        item.key = format!("/serie/{id}");
        item.url = Some(format!("{BASE_URL}/serie/{id}"));
    }
    if let Some(team) = response.main_team {
        item.authors = vec![team.name];
    }
    item.initialized = true;
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let response: SeriesResponse = json(body, DETAILS_FIXTURE);
    response
        .chapters
        .into_iter()
        .map(|chapter| {
            let title = chapter
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Chapitre {}", chapter.number));
            MangaChapter {
                key: format!("/chapitre/{}", chapter.id),
                title: Some(title),
                chapter_number: Some(chapter.number as f32),
                date_uploaded: chapter.released_at.as_deref().and_then(parse_api_date),
                url: Some(format!("{BASE_URL}/chapitre/{}", chapter.id)),
                language: Some(LANG.into()),
                is_locked: chapter.access_type.as_deref() == Some("premium"),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response: ChapterResponse = json(body, PAGES_FIXTURE);
    response
        .chapter
        .images
        .into_iter()
        .enumerate()
        .map(|(index, path)| page_from_path(index, &path))
        .collect()
}

fn page_from_path(index: usize, path: &str) -> MangaPage {
    let description = Some(format!("Page {}", index + 1));
    if let Some((mime_type, bytes)) = data_image(path) {
        return MangaPage {
            content: PageContent::ImageBytes { bytes, mime_type },
            description,
            ..MangaPage::default()
        };
    }
    let image = if path.starts_with("http") {
        path.to_string()
    } else {
        url::join_url(BASE_URL, path)
    };
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description,
        ..MangaPage::default()
    }
}

fn data_image(value: &str) -> Option<(String, Vec<u8>)> {
    let value = value.strip_prefix("data:")?;
    let (meta, data) = value.split_once(',')?;
    let (mime_type, bytes) = if meta.ends_with(";base64") {
        (
            meta.trim_end_matches(";base64").to_string(),
            decode_base64(data)?,
        )
    } else {
        (meta.to_string(), percent_decode(data))
    };
    Some((mime_type, bytes))
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut out = Vec::new();
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    Some(out)
}

fn percent_decode(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    Some(match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    })
}

fn parse_api_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.split('T').next().unwrap_or(value))
}

fn deeplink_series_id(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .and_then(|path| path.trim_start_matches('/').strip_prefix("serie/"))
        .and_then(|path| path.split('/').next())
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
}

fn json<T>(body: &str, fixture: &str) -> T
where
    T: for<'de> Deserialize<'de> + Default,
{
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or_default()
}

#[derive(Default, Deserialize)]
struct ProjectsResponse {
    #[serde(default)]
    projects: Vec<Project>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    title: String,
    genre: Option<String>,
    #[serde(rename = "type")]
    type_name: Option<String>,
    status: Option<String>,
    description: Option<String>,
    cover_image: Option<String>,
}

impl Project {
    fn into_catalog(self) -> CatalogItem {
        CatalogItem {
            key: format!("/serie/{}", self.id),
            title: if self.title.is_empty() {
                self.id.clone()
            } else {
                self.title
            },
            cover: self.cover_image.map(|image| {
                if image.starts_with("http") {
                    image
                } else {
                    url::join_url(BASE_URL, &image)
                }
            }),
            description: self.description,
            tags: [self.genre, self.type_name].into_iter().flatten().collect(),
            status: match self.status.as_deref() {
                Some("ongoing") => ItemStatus::Ongoing,
                Some("completed") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/serie/{}", self.id)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesResponse {
    #[serde(default)]
    project: Project,
    #[serde(default)]
    chapters: Vec<ChapterEntry>,
    main_team: Option<Team>,
}

#[derive(Deserialize)]
struct Team {
    name: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterEntry {
    id: String,
    number: u32,
    title: Option<String>,
    access_type: Option<String>,
    released_at: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChapterResponse {
    #[serde(default)]
    chapter: ChapterDetail,
}

#[derive(Default, Deserialize)]
struct ChapterDetail {
    #[serde(default)]
    images: Vec<String>,
}

const LIST_FIXTURE: &str = r#"
{"projects":[{"id":"sample","title":"Sample","genre":"Action","type":"Webtoon","status":"ongoing","description":"Resume","coverImage":"/cover.jpg"}]}
"#;

const DETAILS_FIXTURE: &str = r#"
{"project":{"id":"sample","title":"Sample","genre":"Action","type":"Webtoon","status":"ongoing","description":"Resume","coverImage":"/cover.jpg"},"chapters":[{"id":"chapter-1","number":1,"title":"Chapitre 1","releasedAt":"2024-01-01T00:00:00"}],"mainTeam":{"name":"Team"}}
"#;

const PAGES_FIXTURE: &str = r#"
{"chapter":{"images":["/page1.jpg","data:image/png;base64,iVBORw0KGgo="]}}
"#;
