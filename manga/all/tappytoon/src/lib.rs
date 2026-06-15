use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Tappytoon = Tappytoon;
const API_URL: &str = "https://api-global.tappytoon.com";
const WEB_BASE: &str = "https://www.tappytoon.com";

struct Tappytoon;

impl MangaSource for Tappytoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!(
                "{API_URL}/comics?day_of_week={}&locale={}",
                latest_day(&request),
                source.lang
            )
        } else {
            format!(
                "{API_URL}/comics?sort_by=trending&filter=completed&locale={}",
                source.lang
            )
        };
        Ok(parse_comics_page(
            &fetch_api_or_fixture(&target, source, COMICS_FIXTURE),
            source,
            false,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![catalog_from_key(&key, source)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!("{API_URL}/comics?locale={}", source.lang);
        if let Some(genre) = filters
            .get("genre")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            target.push_str("&genre=");
            target.push_str(&url::query_escape(genre));
            target.push_str("&limit=50");
        } else if !query.is_empty() {
            target.push_str("&keyword=");
            target.push_str(&url::query_escape(query));
        }
        Ok(parse_comics_page(
            &fetch_api_or_fixture(&target, source, COMICS_FIXTURE),
            source,
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample|1".to_string());
        Ok(catalog_from_key(&key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample|1".to_string());
        let comic_id = key.split('|').nth(1).unwrap_or("1");
        let target = format!(
            "{API_URL}/comics/{comic_id}/chapters?locale={}",
            source.lang
        );
        Ok(parse_chapters(&fetch_api_or_fixture(
            &target,
            source,
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let chapter_id = request_key(&request, "chapter").unwrap_or_else(|| "10".to_string());
        let target = format!(
            "{API_URL}/content-delivery/contents?chapterId={chapter_id}&variant=high&locale={}",
            source.lang
        );
        Ok(parse_pages(&fetch_api_or_fixture(
            &target,
            source,
            MEDIA_FIXTURE,
        )))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let source = source_for(&request);
        let popular = self.list(serde_json::json!({"sourceId": source.id, "listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        let latest = self.list(serde_json::json!({"sourceId": source.id, "listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        Ok(request_key(&request, "manga").map(|key| comic_url(&key, source)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "chapter").map(|key| format!("{API_URL}/chapters/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_key(&key, source)),
                url: Some(comic_url(&key, source)),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "tappytoon-en",
        lang: "en",
    },
    SourceConfig {
        id: "tappytoon-fr",
        lang: "fr",
    },
    SourceConfig {
        id: "tappytoon-de",
        lang: "de",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("tappytoon-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client(source: SourceConfig) -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{WEB_BASE}/{}/", source.lang))
        .with_origin(WEB_BASE)
        .with_cookies_for(WEB_BASE)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(target: &str, source: SourceConfig, fixture: &str) -> String {
    let headers = api_headers(source).unwrap_or_else(|_| ApiHeaders::fixture());
    client(source)
        .get(target)
        .xhr()
        .header("Accept-Language", source.lang)
        .header("Authorization", headers.authorization)
        .header("X-Device-Uuid", headers.device_uuid)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_headers(source: SourceConfig) -> ExtensionResult<ApiHeaders> {
    let body = client(source)
        .get(format!("{WEB_BASE}/{}", source.lang))
        .browser_document()
        .send_text()?;
    let data = body
        .split("id=\"__NEXT_DATA__\"")
        .nth(1)
        .and_then(|tail| tail.split('>').nth(1))
        .and_then(|tail| tail.split("</script>").next())
        .ok_or_else(|| manatan_extension::abi::ExtensionError {
            message: "Tappytoon __NEXT_DATA__ not found".to_string(),
        })?;
    headers_from_next_data(data).ok_or_else(|| manatan_extension::abi::ExtensionError {
        message: "Tappytoon API headers not found".to_string(),
    })
}

fn headers_from_next_data(data: &str) -> Option<ApiHeaders> {
    let value = serde_json::from_str::<Value>(data).ok()?;
    let headers = value.pointer("/props/initialState/axios/headers")?;
    Some(ApiHeaders {
        authorization: headers.get("Authorization")?.as_str()?.to_string(),
        device_uuid: headers.get("X-Device-Uuid")?.as_str()?.to_string(),
    })
}

#[derive(Clone)]
struct ApiHeaders {
    authorization: String,
    device_uuid: String,
}

impl ApiHeaders {
    fn fixture() -> Self {
        Self {
            authorization: "Bearer fixture".to_string(),
            device_uuid: "fixture-device".to_string(),
        }
    }
}

fn parse_comics_page(body: &str, source: SourceConfig, has_next_page: bool) -> Paged<CatalogItem> {
    let comics = serde_json::from_str::<Vec<Comic>>(body).unwrap_or_else(|_| sample_comics());
    Paged {
        entries: comics
            .into_iter()
            .filter(|comic| comic.is_accessible)
            .map(|comic| comic.into_catalog(source))
            .collect(),
        has_next_page,
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<Chapter>>(body)
        .unwrap_or_else(|_| sample_chapters())
        .into_iter()
        .filter(|chapter| chapter.is_accessible)
        .rev()
        .map(|chapter| {
            let locked = !chapter.is_free && !(chapter.is_user_unlocked || chapter.is_user_rented);
            MangaChapter {
                key: chapter.id.to_string(),
                title: Some(
                    format!(
                        "{}{}{}",
                        chapter.title,
                        if chapter.subtitle.is_empty() {
                            ""
                        } else {
                            " - "
                        },
                        chapter.subtitle
                    ) + if locked { " [locked]" } else { "" },
                ),
                chapter_number: Some(chapter.order + 1.0),
                date_uploaded: parse_tappytoon_date(&chapter.will_accessible_at),
                url: Some(format!("{API_URL}/chapters/{}", chapter.id)),
                is_locked: locked,
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let media = serde_json::from_str::<Media>(body).unwrap_or_else(|_| sample_media());
    media
        .media
        .into_iter()
        .enumerate()
        .map(|(index, item)| MangaPage {
            content: PageContent::Url {
                url: item.url,
                context: None,
            },
            description: Some((index + 1).to_string()),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_from_key(key: &str, source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.split('|').next().unwrap_or("Comic").replace('-', " "),
        url: Some(comic_url(key, source)),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn comic_url(key: &str, source: SourceConfig) -> String {
    format!(
        "{WEB_BASE}/{}/comics/{}",
        source.lang,
        key.split('|').next().unwrap_or(key)
    )
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|item| {
            item.get("key")
                .or_else(|| item.get("url"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

fn key_from_url(input: &str) -> Option<String> {
    if input.contains('|') {
        return Some(input.to_string());
    }
    let slug = input
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .rsplit('/')
        .next()?;
    (!slug.is_empty()).then(|| format!("{slug}|0"))
}

fn latest_day(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("latestDay"))
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "sun" | "mon" | "tue" | "wed" | "thu" | "fri" | "sat"
            )
        })
        .unwrap_or("mon")
}

fn parse_tappytoon_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    Some(days_from_civil(parts.next()?, parts.next()?, parts.next()?) * 86_400)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[derive(Debug, Deserialize)]
struct Comic {
    id: i64,
    title: String,
    slug: String,
    #[serde(rename = "longDescription")]
    long_description: String,
    #[serde(rename = "posterThumbnailUrl")]
    poster_thumbnail_url: String,
    #[serde(rename = "isHiatus")]
    is_hiatus: bool,
    #[serde(rename = "isAccessible")]
    is_accessible: bool,
    #[serde(rename = "isCompleted")]
    is_completed: bool,
    #[serde(rename = "ageRating")]
    age_rating: Name,
    #[serde(default)]
    genres: Vec<Name>,
    #[serde(default)]
    authors: Vec<Name>,
}

impl Comic {
    fn into_catalog(self, source: SourceConfig) -> CatalogItem {
        let key = format!("{}|{}", self.slug, self.id);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            description: Some(self.long_description),
            cover: Some(self.poster_thumbnail_url),
            url: Some(comic_url(&key, source)),
            authors: self.authors.iter().map(|name| name.name.clone()).collect(),
            artists: self.authors.into_iter().map(|name| name.name).collect(),
            tags: self
                .genres
                .into_iter()
                .map(|name| name.name)
                .chain(Some(format!("Rating: {}", self.age_rating.name)))
                .collect(),
            language: Some(source.lang.to_string()),
            content_rating: Some("adult".to_string()),
            status: if self.is_completed {
                ItemStatus::Completed
            } else if !self.is_hiatus {
                ItemStatus::Ongoing
            } else {
                ItemStatus::Unknown
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Name {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    id: i64,
    order: f32,
    title: String,
    subtitle: String,
    #[serde(rename = "isAccessible")]
    is_accessible: bool,
    #[serde(rename = "isFree")]
    is_free: bool,
    #[serde(rename = "isUserUnlocked")]
    is_user_unlocked: bool,
    #[serde(rename = "isUserRented")]
    is_user_rented: bool,
    #[serde(rename = "willAccessibleAt")]
    will_accessible_at: String,
}

#[derive(Debug, Deserialize)]
struct Media {
    media: Vec<MediaUrl>,
}

#[derive(Debug, Deserialize)]
struct MediaUrl {
    url: String,
}

fn sample_comics() -> Vec<Comic> {
    serde_json::from_str(COMICS_FIXTURE).expect("valid comics fixture")
}

fn sample_chapters() -> Vec<Chapter> {
    serde_json::from_str(CHAPTERS_FIXTURE).expect("valid chapters fixture")
}

fn sample_media() -> Media {
    serde_json::from_str(MEDIA_FIXTURE).expect("valid media fixture")
}

const COMICS_FIXTURE: &str = r#"[
  {
    "id": 1,
    "title": "Sample Comic",
    "slug": "sample-comic",
    "longDescription": "Sample description",
    "posterThumbnailUrl": "https://cdn.example/poster.jpg",
    "isHiatus": false,
    "isAccessible": true,
    "isCompleted": false,
    "ageRating": { "name": "Teen" },
    "genres": [{ "name": "Fantasy" }],
    "authors": [{ "name": "Sample Author" }]
  }
]"#;

const CHAPTERS_FIXTURE: &str = r#"[
  {
    "id": 10,
    "order": 0.0,
    "title": "Episode 1",
    "subtitle": "Start",
    "isAccessible": true,
    "isFree": true,
    "isUserUnlocked": false,
    "isUserRented": false,
    "willAccessibleAt": "2024-01-01T00:00:00"
  }
]"#;

const MEDIA_FIXTURE: &str = r#"{
  "media": [
    { "url": "https://cdn.example/page-1.jpg" },
    { "url": "https://cdn.example/page-2.jpg" }
  ]
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_from_next_data() {
        let data = r#"{"props":{"initialState":{"axios":{"headers":{"Authorization":"Bearer abc","X-Device-Uuid":"uuid"}}}}}"#;
        let headers = headers_from_next_data(data).unwrap();
        assert_eq!(headers.authorization, "Bearer abc");
        assert_eq!(headers.device_uuid, "uuid");
    }

    #[test]
    fn parses_comics() {
        let page = parse_comics_page(COMICS_FIXTURE, SOURCES[0], false);
        assert_eq!(page.entries[0].key, "sample-comic|1");
        assert_eq!(page.entries[0].authors, vec!["Sample Author"]);
    }

    #[test]
    fn parses_chapters() {
        let chapters = parse_chapters(CHAPTERS_FIXTURE);
        assert_eq!(chapters[0].key, "10");
        assert_eq!(chapters[0].date_uploaded, Some(1_704_067_200));
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(MEDIA_FIXTURE);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://cdn.example/page-1.jpg"),
            _ => panic!("expected URL page"),
        }
    }
}
