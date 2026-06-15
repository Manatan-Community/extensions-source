use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Nekopost = Nekopost;
const BASE_URL: &str = "https://www.nekopost.net";
const PROJECT_ENDPOINT: &str = "https://api.osemocphoto.com/frontAPI/getProjectInfo";
const FILE_HOST: &str = "https://www.osemocphoto.com";
const POPULAR_PAGE_SIZE: u64 = 15;
const LATEST_PAGE_SIZE: u64 = 15;
const SEARCH_PAGE_SIZE: u64 = 100;

struct Nekopost;

impl MangaSource for Nekopost {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            let body = fetch_post_json_or_fixture(
                &format!("{BASE_URL}/api/project/latest"),
                json!({"type":"m","paging":{"pageNo":page,"pageSize":LATEST_PAGE_SIZE}}),
                LATEST_FIXTURE,
            );
            let latest = serde_json::from_str::<RawLatestChapterList>(&body).unwrap_or_default();
            let entries = latest
                .list_chapter
                .unwrap_or_default()
                .into_iter()
                .map(CatalogItem::from)
                .collect::<Vec<_>>();
            return Ok(Paged {
                has_next_page: entries.len() as u64 == LATEST_PAGE_SIZE,
                entries,
            });
        }
        let body = fetch_post_json_or_fixture(
            &format!("{BASE_URL}/api/project/list/popular"),
            json!({"type":"mc","paging":{"pageNo":1,"pageSize":POPULAR_PAGE_SIZE}}),
            SEARCH_FIXTURE,
        );
        Ok(project_list_page(&body, None, false))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(project_id) = id_after(query, "nekopost.net/manga/") {
            let body =
                fetch_get_or_fixture(&format!("{PROJECT_ENDPOINT}/{project_id}"), DETAILS_FIXTURE);
            let item = serde_json::from_str::<RawProjectInfo>(&body)
                .ok()
                .and_then(project_info_item);
            return Ok(Paged {
                entries: item.into_iter().collect(),
                has_next_page: false,
            });
        }
        if let Some(editor_id) = id_after(query, "nekopost.net/editor/") {
            let body = fetch_get_or_fixture(
                &format!("{BASE_URL}/api/editor/project/{editor_id}"),
                EDITOR_FIXTURE,
            );
            let entries = serde_json::from_str::<Vec<EditorProject>>(&body)
                .unwrap_or_default()
                .into_iter()
                .filter(|item| item.project_type == "m")
                .map(CatalogItem::from)
                .collect();
            return Ok(Paged {
                entries,
                has_next_page: false,
            });
        }
        let body = fetch_post_json_or_fixture(
            &format!("{BASE_URL}/api/project/search"),
            json!({"keyword":query,"status":0,"paging":{"pageNo":page,"pageSize":SEARCH_PAGE_SIZE}}),
            SEARCH_FIXTURE,
        );
        Ok(project_list_page(&body, Some("m"), true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let body = fetch_get_or_fixture(&format!("{PROJECT_ENDPOINT}/{key}"), DETAILS_FIXTURE);
        Ok(serde_json::from_str::<RawProjectInfo>(&body)
            .ok()
            .and_then(project_info_item)
            .unwrap_or_else(sample_item))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        let body = fetch_get_or_fixture(&format!("{PROJECT_ENDPOINT}/{key}"), DETAILS_FIXTURE);
        let info = serde_json::from_str::<RawProjectInfo>(&body).unwrap_or_default();
        let project_id = info
            .project_info
            .as_ref()
            .map(|item| item.project_id.clone())
            .unwrap_or(key);
        Ok(info
            .project_chapter_list
            .unwrap_or_default()
            .into_iter()
            .filter_map(|chapter| chapter_item(&project_id, chapter))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "1/1/1_1.json".to_string());
        let body = fetch_get_or_fixture(&format!("{FILE_HOST}/collectManga/{key}"), PAGES_FIXTURE);
        let chapter = serde_json::from_str::<RawChapterInfo>(&body).unwrap_or_default();
        let base = format!(
            "{FILE_HOST}/collectManga/{}/{}",
            chapter.project_id, chapter.chapter_id
        );
        Ok(chapter
            .page_item
            .into_iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let image = page.page_name.or(page.file_name)?;
                Some(MangaPage {
                    content: PageContent::Url {
                        url: format!("{base}/{image}"),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let mut parts = key.split('/');
            let project = parts.next().unwrap_or_default();
            let chapter_json = parts.nth(1).unwrap_or_default();
            let chapter = chapter_json
                .trim_end_matches(".json")
                .rsplit('_')
                .next()
                .unwrap_or_default();
            format!("{BASE_URL}/manga/{project}/{chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
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
        .with_header("Accept", "*/*")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn fetch_get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_post_json_or_fixture(target: &str, body: Value, fixture: &str) -> String {
    client()
        .post(target)
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn project_list_page(
    body: &str,
    project_type: Option<&str>,
    paginated: bool,
) -> Paged<CatalogItem> {
    let list = serde_json::from_str::<RawProjectSearchSummaryList>(body).unwrap_or_default();
    let entries = list
        .list_project
        .unwrap_or_default()
        .into_iter()
        .filter(|item| project_type.is_none_or(|kind| item.project_type == kind))
        .map(CatalogItem::from)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: paginated && entries.len() as u64 == SEARCH_PAGE_SIZE,
        entries,
    }
}

fn project_info_item(info: RawProjectInfo) -> Option<CatalogItem> {
    let project = info.project_info?;
    Some(CatalogItem {
        key: project.project_id.clone(),
        title: project.project_name,
        cover: Some(build_cover_url(&project.project_id, None)),
        authors: non_empty_vec(project.author_name),
        artists: non_empty_vec(project.artist_name),
        description: (!project.info.is_empty()).then_some(project.info),
        tags: info
            .project_category_used
            .unwrap_or_default()
            .into_iter()
            .map(|category| category.category_name)
            .collect(),
        status: parse_status(&project.status),
        url: Some(format!("{BASE_URL}/manga/{}", project.project_id)),
        language: Some("th".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn chapter_item(project_id: &str, chapter: RawProjectChapter) -> Option<MangaChapter> {
    let chapter_id = chapter.chapter_id?;
    let key = format!("{project_id}/{chapter_id}/{project_id}_{chapter_id}.json");
    Some(MangaChapter {
        key: key.clone(),
        title: Some(chapter.chapter_name),
        chapter_number: chapter.chapter_no.parse().ok(),
        date_uploaded: dates::parse_ymd(chapter.create_date.get(..10).unwrap_or_default()),
        scanlators: non_empty_vec(chapter.provider_name),
        url: Some(format!(
            "{BASE_URL}/manga/{}/{}",
            project_id, chapter.chapter_no
        )),
        ..MangaChapter::default()
    })
}

fn build_cover_url(project_id: &str, cover_version: Option<i64>) -> String {
    let base = format!("{FILE_HOST}/collectManga/{project_id}/{project_id}_cover.jpg");
    cover_version
        .map(|version| format!("{base}?ver={version}"))
        .unwrap_or(base)
}

fn parse_status(status: &str) -> ItemStatus {
    match status {
        "1" => ItemStatus::Ongoing,
        "2" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn id_after(input: &str, marker: &str) -> Option<String> {
    input
        .split_once(marker)?
        .1
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn non_empty_vec(value: String) -> Vec<String> {
    if value.trim().is_empty() {
        Vec::new()
    } else {
        vec![value]
    }
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "1".to_string(),
        title: "Sample".to_string(),
        language: Some("th".to_string()),
        content_rating: Some("adult".to_string()),
        ..CatalogItem::default()
    }
}

impl From<RawProjectSearchSummary> for CatalogItem {
    fn from(item: RawProjectSearchSummary) -> Self {
        CatalogItem {
            key: item.pid.to_string(),
            title: item.project_name,
            cover: Some(build_cover_url(
                &item.pid.to_string(),
                Some(item.cover_version),
            )),
            status: parse_status(&item.status.to_string()),
            url: Some(format!("{BASE_URL}/manga/{}", item.pid)),
            language: Some("th".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

impl From<RawLatestChapter> for CatalogItem {
    fn from(item: RawLatestChapter) -> Self {
        CatalogItem {
            key: item.pid.to_string(),
            title: item.project_name,
            cover: Some(build_cover_url(
                &item.pid.to_string(),
                Some(item.cover_version),
            )),
            status: parse_status(&item.status),
            url: Some(format!("{BASE_URL}/manga/{}", item.pid)),
            language: Some("th".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

impl From<EditorProject> for CatalogItem {
    fn from(item: EditorProject) -> Self {
        CatalogItem {
            key: item.pid.to_string(),
            title: item.project_name,
            cover: Some(build_cover_url(&item.pid.to_string(), item.cover_version)),
            status: parse_status(&item.status.to_string()),
            url: Some(format!("{BASE_URL}/manga/{}", item.pid)),
            language: Some("th".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawLatestChapterList {
    list_chapter: Option<Vec<RawLatestChapter>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLatestChapter {
    pid: i64,
    project_name: String,
    cover_version: i64,
    status: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawProjectSearchSummaryList {
    list_project: Option<Vec<RawProjectSearchSummary>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProjectSearchSummary {
    pid: i64,
    project_name: String,
    status: i64,
    project_type: String,
    cover_version: i64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawProjectInfo {
    #[serde(rename = "listCate")]
    project_category_used: Option<Vec<RawProjectCategory>>,
    #[serde(rename = "listChapter")]
    project_chapter_list: Option<Vec<RawProjectChapter>>,
    #[serde(rename = "projectInfo")]
    project_info: Option<RawProjectInfoData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProjectInfoData {
    project_id: String,
    project_name: String,
    author_name: String,
    artist_name: String,
    info: String,
    status: String,
}

#[derive(Deserialize)]
struct RawProjectCategory {
    #[serde(rename = "cateName")]
    category_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProjectChapter {
    chapter_id: Option<String>,
    chapter_no: String,
    chapter_name: String,
    create_date: String,
    provider_name: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawChapterInfo {
    chapter_id: i64,
    page_item: Vec<RawPageItem>,
    project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPageItem {
    page_name: Option<String>,
    file_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditorProject {
    pid: i64,
    project_name: String,
    project_type: String,
    status: i64,
    cover_version: Option<i64>,
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"listProject":[{"pid":1,"projectName":"Sample","status":1,"projectType":"m","coverVersion":1}]}"#;
const LATEST_FIXTURE: &str =
    r#"{"listChapter":[{"pid":1,"projectName":"Sample","coverVersion":1,"status":"1"}]}"#;
const EDITOR_FIXTURE: &str =
    r#"[{"pid":1,"projectName":"Sample","projectType":"m","status":1,"coverVersion":1}]"#;
const DETAILS_FIXTURE: &str = r#"{"listCate":[{"cateName":"Action"}],"listChapter":[{"chapterId":"1","chapterNo":"1","chapterName":"Chapter 1","createDate":"2024-01-01 00:00:00","providerName":"Nekopost"}],"projectInfo":{"projectId":"1","projectName":"Sample","authorName":"Author","artistName":"Artist","info":"Sample description.","status":"1"}}"#;
const PAGES_FIXTURE: &str = r#"{"chapterId":1,"chapterNo":"1","projectId":"1","pageItem":[{"pageName":"001.jpg","height":1,"pageNo":1,"width":1}]}"#;
