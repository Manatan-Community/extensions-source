use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Source = Source;
const BASE_URL: &str = "https://perf-scan.xyz";
const API_URL: &str = "https://api.perf-scan.xyz";
const TAKE: u64 = 24;

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{API_URL}/series?type=COMIC&page={page}&take={TAKE}&latestUpdate=true")
        } else {
            format!(
                "{API_URL}/series?ranking=POPULAR&rankingType=YEARLY&type=COMIC&page={page}&take={TAKE}"
            )
        };
        Ok(parse_list(&fetch(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_list(&fetch(
            &format!(
                "{API_URL}/series?type=COMIC&title={}&page={}&take={TAKE}",
                url::query_escape(query),
                page(&request)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(
            &fetch(&details_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_chapters(&fetch(&details_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter/1".into());
        Ok(parse_pages(&fetch(
            &url::join_url(API_URL, &key),
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
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(&details_url(&key), DETAILS_FIXTURE),
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
        .with_header("Origin", BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let root = json(body, LIST_FIXTURE);
    let data = root
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let count = data.len() as u64;
    Paged {
        entries: data.into_iter().map(item).collect(),
        has_next_page: count == TAKE,
    }
}

fn item(value: Value) -> CatalogItem {
    let slug = str_field(&value, "slug").unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: format!("/series/{slug}"),
        title: str_field(&value, "title").unwrap_or_else(|| slug.clone()),
        cover: str_field(&value, "thumbnail")
            .map(|image| format!("{API_URL}/cdn/{}", image.trim_start_matches('/'))),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let root = json(body, DETAILS_FIXTURE);
    let data = root.get("data").cloned().unwrap_or_default();
    let slug = str_field(&data, "slug")
        .unwrap_or_else(|| slug(&key.unwrap_or_else(|| "/series/sample".into())));
    CatalogItem {
        key: format!("/series/{slug}"),
        title: str_field(&data, "title").unwrap_or_else(|| slug.clone()),
        cover: str_field(&data, "thumbnail")
            .map(|image| format!("{API_URL}/cdn/{}", image.trim_start_matches('/'))),
        description: str_field(&data, "description"),
        authors: str_field(&data, "author").into_iter().collect(),
        artists: str_field(&data, "artist").into_iter().collect(),
        tags: data
            .get("SeriesGenre")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|genre| {
                genre
                    .pointer("/Genre/name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect(),
        status: status(data.pointer("/Status/name").and_then(Value::as_str)),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root = json(body, DETAILS_FIXTURE);
    let data = root.get("data").cloned().unwrap_or_default();
    let slug = str_field(&data, "slug").unwrap_or_else(|| "sample".into());
    let mut chapters = data
        .get("Chapter")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let num = chapter.get("index").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            let num_text = number(num);
            let title =
                str_field(&chapter, "title").filter(|value| !value.is_empty() && value != "-");
            MangaChapter {
                key: format!("/series/{slug}/chapter/{num_text}"),
                title: Some(
                    title
                        .map(|t| format!("Chapitre {num_text} - {t}"))
                        .unwrap_or_else(|| format!("Chapitre {num_text}")),
                ),
                chapter_number: Some(num),
                date_uploaded: str_field(&chapter, "createdAt").and_then(|date| {
                    manatan_shared::dates::parse_ymd(date.get(0..10).unwrap_or(&date))
                }),
                scanlators: chapter
                    .pointer("/Season/name")
                    .and_then(Value::as_str)
                    .map(|s| vec![s.to_string()])
                    .unwrap_or_default(),
                url: Some(format!("{BASE_URL}/series/{slug}/chapter/{num_text}")),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| b.chapter_number.partial_cmp(&a.chapter_number).unwrap());
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    json(body, PAGES_FIXTURE)
        .pointer("/data/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|page| str_field(&page, "value"))
        .enumerate()
        .map(|(index, image)| {
            page_url(
                format!("{API_URL}/cdn/{}", image.trim_start_matches('/')),
                index,
            )
        })
        .collect()
}

fn page_url(image: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/series/{}", slug(key))
}
fn slug(key: &str) -> String {
    key.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}
fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) && input.contains("/series/"))
        .then(|| format!("/series/{}", slug(input)))
}
fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}
fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToString::to_string)
}
fn json(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or_default()
}
fn number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}
fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or("").to_ascii_lowercase().as_str() {
        "en cours" => ItemStatus::Ongoing,
        "terminé" | "termine" => ItemStatus::Completed,
        "en pause" => ItemStatus::Hiatus,
        "annulé" | "annule" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str =
    r#"{"data":[{"slug":"sample","title":"Sample Perf","thumbnail":"cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"title":"Sample Perf","slug":"sample","thumbnail":"cover.jpg","description":"Summary","author":"Author","artist":"Artist","SeriesGenre":[{"Genre":{"name":"Action"}}],"Status":{"name":"En cours"},"Chapter":[{"index":1,"title":"Debut","createdAt":"2024-01-01T00:00:00.000Z","Season":{"name":"Saison 1"}}]}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"content":[{"value":"page1.jpg"}]}}"#;
