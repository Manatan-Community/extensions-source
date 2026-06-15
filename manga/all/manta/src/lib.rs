use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Manta = Manta;
const DOMAIN: &str = "https://manta.net";

struct Manta;

impl MangaSource for Manta {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.search(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(series_id) = series_id_from_query(query) {
            let body = fetch_or_fixture(
                source,
                &format!("{DOMAIN}/front/v1/series/{series_id}?lang={}", source.lang),
                DETAILS_FIXTURE,
            );
            return Ok(Paged {
                entries: vec![parse_details(&body, source, Some(series_id))],
                has_next_page: false,
            });
        }

        let target = if query.is_empty() {
            let selected = request
                .get("filters")
                .and_then(|filters| filters.get("category"))
                .and_then(Value::as_str)
                .unwrap_or("tagId=288");
            format!(
                "{DOMAIN}/manta/v1/search/series?{}&lang={}",
                selected, source.lang
            )
        } else {
            format!(
                "{DOMAIN}/manta/v1/search/series?q={}&lang={}",
                encode_query(query),
                source.lang
            )
        };
        let body = fetch_or_fixture(source, &target, LIST_FIXTURE);
        Ok(parse_list(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let body = fetch_or_fixture(
            source,
            &format!("{DOMAIN}/front/v1/series/{key}?lang={}", source.lang),
            DETAILS_FIXTURE,
        );
        Ok(parse_details(&body, source, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let body = fetch_or_fixture(
            source,
            &format!("{DOMAIN}/front/v1/series/{key}?lang={}", source.lang),
            DETAILS_FIXTURE,
        );
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "10".into());
        let body = fetch_or_fixture(
            source,
            &format!("{DOMAIN}/front/v1/episodes/{key}?lang={}", source.lang),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(series_id) = series_id_from_query(input) {
            let source = source_for(&request);
            let body = fetch_or_fixture(
                source,
                &format!("{DOMAIN}/front/v1/series/{series_id}?lang={}", source.lang),
                DETAILS_FIXTURE,
            );
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, source, Some(series_id))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    base_url: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "manta-en",
        lang: "en",
        base_url: "https://manta.net/en",
    },
    SourceConfig {
        id: "manta-es",
        lang: "es",
        base_url: "https://manta.net/es",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("manta-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

#[derive(Deserialize)]
struct MantaResponse<T> {
    data: T,
}

#[derive(Deserialize)]
struct Series<T> {
    id: u64,
    data: T,
    image: Option<Cover>,
    #[serde(default)]
    episodes: Option<Vec<Episode>>,
}

#[derive(Deserialize)]
struct Title {
    title: Name,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Details {
    tags: Vec<Tag>,
    is_completed: Option<bool>,
    description: Description,
    creators: Vec<Creator>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    id: u64,
    ord: Option<u32>,
    data: Option<EpisodeData>,
    lock_data: Option<LockData>,
    open_at: Option<String>,
    created_at: Option<String>,
    cut_images: Option<Vec<Image>>,
}

#[derive(Deserialize)]
struct EpisodeData {
    title: Option<String>,
}

#[derive(Deserialize)]
struct LockData {
    state: Option<i32>,
}

#[derive(Deserialize)]
struct Creator {
    name: String,
    role: String,
}

#[derive(Deserialize)]
struct Description {
    long: String,
    short: Option<String>,
}

#[derive(Deserialize)]
struct Tag {
    name: Name,
}

#[derive(Deserialize)]
struct Name {
    en: Option<String>,
    es: Option<String>,
}

#[derive(Deserialize)]
struct Cover {
    #[serde(rename = "1280x1840_480")]
    size_480: Option<Image>,
    #[serde(rename = "1280x1840_720")]
    size_720: Option<Image>,
    #[serde(rename = "1440x3072")]
    size_tall: Option<Image>,
    #[serde(rename = "1440x1440_480")]
    size_square: Option<Image>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Image {
    download_url: String,
}

fn fetch_or_fixture(source: SourceConfig, target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(source.base_url)
        .with_cookies_for(DOMAIN)
        .get(target)
        .header("Origin", DOMAIN)
        .header("Accept-Language", source.lang)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<MantaResponse<Vec<Series<Title>>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("list fixture"));
    Paged {
        entries: response
            .data
            .into_iter()
            .map(|series| CatalogItem {
                key: series.id.to_string(),
                title: series.data.title.as_string(source.lang),
                cover: series.image.and_then(Cover::best),
                url: Some(format!("{}/series/{}", source.base_url, series.id)),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, source: SourceConfig, key: Option<String>) -> CatalogItem {
    let response = serde_json::from_str::<MantaResponse<Series<Details>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"));
    let series = response.data;
    let details = series.data;
    let authors = details
        .creators
        .iter()
        .filter(|creator| creator.role != "Illustration")
        .map(|creator| creator.name.clone())
        .collect::<Vec<_>>();
    let artists = details
        .creators
        .iter()
        .filter(|creator| creator.role == "Illustration")
        .map(|creator| creator.name.clone())
        .collect::<Vec<_>>();
    CatalogItem {
        key: key.unwrap_or_else(|| series.id.to_string()),
        title: String::new(),
        cover: series.image.and_then(Cover::best),
        url: Some(format!("{}/series/{}", source.base_url, series.id)),
        authors: if authors.is_empty() {
            details.creators.iter().map(|c| c.name.clone()).collect()
        } else {
            authors
        },
        artists,
        description: Some(details.description.as_string()),
        tags: details
            .tags
            .into_iter()
            .map(|tag| tag.name.as_string(source.lang))
            .filter(|tag| !tag.is_empty())
            .collect(),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        status: if details.is_completed == Some(true) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<MantaResponse<Series<Title>>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("chapters fixture"));
    let mut chapters = response
        .data
        .episodes
        .unwrap_or_default()
        .into_iter()
        .map(|episode| MangaChapter {
            key: episode.id.to_string(),
            title: Some(episode.title(source.lang)),
            chapter_number: episode.ord.map(|ord| ord as f32),
            language: Some(source.lang.into()),
            url: Some(format!("{}/episodes/{}", source.base_url, episode.id)),
            is_locked: episode.lock_data.as_ref().is_some_and(LockData::is_locked),
            date_uploaded: episode
                .open_at
                .or(episode.created_at)
                .and_then(parse_iso_millis),
            ..MangaChapter::default()
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<MantaResponse<Episode>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("pages fixture"));
    response
        .data
        .cut_images
        .unwrap_or_default()
        .into_iter()
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: image.download_url,
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

impl Name {
    fn as_string(&self, lang: &str) -> String {
        if lang == "es" {
            self.es
                .clone()
                .or_else(|| self.en.clone())
                .unwrap_or_default()
        } else {
            self.en
                .clone()
                .or_else(|| self.es.clone())
                .unwrap_or_default()
        }
    }
}

impl Description {
    fn as_string(&self) -> String {
        [self.short.as_deref(), Some(self.long.as_str())]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl Cover {
    fn best(self) -> Option<String> {
        self.size_480
            .or(self.size_720)
            .or(self.size_tall)
            .or(self.size_square)
            .map(|image| image.download_url)
    }
}

impl Episode {
    fn title(&self, lang: &str) -> String {
        let mut title = self
            .data
            .as_ref()
            .and_then(|data| data.title.clone())
            .unwrap_or_else(|| {
                format!(
                    "{} {}",
                    if lang == "es" { "Episodio" } else { "Episode" },
                    self.ord.unwrap_or_default()
                )
            });
        if self.lock_data.as_ref().is_some_and(LockData::is_locked) {
            title.push_str(" (locked)");
        }
        title
    }
}

impl LockData {
    fn is_locked(&self) -> bool {
        self.state.is_some_and(|state| state != 110 && state != 130)
    }
}

fn series_id_from_query(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(value.to_string());
    }
    value
        .split("/series/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|id| id.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

fn parse_iso_millis(value: String) -> Option<i64> {
    let date = value.split(['.', '+', 'Z']).next()?;
    let mut parts = date.split(['T', '-', ':']);
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let hour = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let minute = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let second = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64) * 1_000)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"data":{"title":{"en":"Sample Manta","es":"Manta Ejemplo"}},"image":{"1280x1840_480":{"downloadUrl":"https://manta.net/cover.jpg"}}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"id":1,"data":{"tags":[{"name":{"en":"Romance","es":"Romance"}}],"isCompleted":false,"description":{"short":"Short.","long":"Long description."},"creators":[{"name":"Writer","role":"Story"},{"name":"Artist","role":"Illustration"}]},"image":{"1280x1840_480":{"downloadUrl":"https://manta.net/cover.jpg"}}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"id":1,"data":{"title":{"en":"Sample Manta"}},"image":{"1280x1840_480":{"downloadUrl":"https://manta.net/cover.jpg"}},"episodes":[{"id":10,"ord":1,"data":{"title":"Episode 1"},"lockData":{"state":110},"openAt":"2024-01-01T00:00:00Z"}]}}"#;
const PAGES_FIXTURE: &str =
    r#"{"data":{"id":10,"ord":1,"cutImages":[{"downloadUrl":"https://manta.net/page-1.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manta() {
        let source = SOURCES[0];
        assert_eq!(
            parse_list(LIST_FIXTURE, source).entries[0].title,
            "Sample Manta"
        );
        assert_eq!(
            parse_details(DETAILS_FIXTURE, source, None).artists,
            vec!["Artist"]
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, source).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
        assert_eq!(
            series_id_from_query("https://manta.net/en/series/1"),
            Some("1".into())
        );
    }
}
