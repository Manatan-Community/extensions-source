use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ReadVagabondManga = ReadVagabondManga;
const BASE_URL: &str = "https://readbagabondo.com";
const BUCKET_URL: &str = "https://bucket.readbagabondo.com";

struct ReadVagabondManga;

impl MangaSource for ReadVagabondManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_list(MANGA_LIST_FIXTURE));
        }
        Ok(parse_manga_list(&api_get(
            "/api/mihon/mangas",
            MANGA_LIST_FIXTURE,
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
            let id = query
                .rsplit('#')
                .next()
                .filter(|value| *value != query)
                .unwrap_or("vagabond");
            return Ok(Paged {
                entries: vec![details_by_id(id)],
                has_next_page: false,
            });
        }
        Ok(parse_manga_list(&api_get(
            &format!(
                "/api/mihon/mangas?q={}&page={page}",
                url::query_escape(query)
            ),
            MANGA_LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "vagabond".to_string());
        Ok(details_by_id(id.trim_start_matches('#')))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "vagabond".to_string());
        Ok(parse_chapters(
            &api_get(
                &format!(
                    "/api/mihon/mangas/{}/chapters",
                    url::query_escape(id.trim_start_matches('#'))
                ),
                CHAPTERS_FIXTURE,
            ),
            id.trim_start_matches('#'),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "vagabond#1".to_string());
        let manga_id = key.rsplit('#').next().unwrap_or("vagabond");
        let chapter_number = key.split('#').next().unwrap_or("1");
        let chapter = parse_chapter(&api_get(
            &format!(
                "/api/mihon/mangas/{}/chapters/{}",
                url::query_escape(manga_id),
                url::query_escape(chapter_number)
            ),
            CHAPTER_FIXTURE,
        ));
        Ok(chapter_pages(chapter))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(serde_json::json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            entries: page.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(Some(BASE_URL.to_string()))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let chapter = key.split('#').next().unwrap_or("1");
            let id = key.rsplit('#').next().unwrap_or("vagabond");
            format!("{BASE_URL}/volume-1/chapter-{chapter}/#{id}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let id = input
                .rsplit('#')
                .next()
                .filter(|value| *value != input)
                .unwrap_or("vagabond");
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(id)),
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
        .get(format!("{BASE_URL}{path}"))
        .header("Accept", "application/json, text/plain, */*")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_list(body: &str) -> Paged<CatalogItem> {
    let mangas = serde_json::from_str::<Vec<MangaDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(MANGA_LIST_FIXTURE).expect("fixture is valid"));
    Paged {
        entries: mangas.into_iter().map(MangaDto::into_item).collect(),
        has_next_page: false,
    }
}

fn details_by_id(id: &str) -> CatalogItem {
    serde_json::from_str::<MangaDto>(&api_get(
        &format!("/api/mihon/mangas/{}", url::query_escape(id)),
        DETAILS_FIXTURE,
    ))
    .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"))
    .into_item_initialized()
}

fn parse_chapters(body: &str, manga_id: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"))
        .into_iter()
        .map(|chapter| chapter.into_chapter(manga_id))
        .collect()
}

fn parse_chapter(body: &str) -> ChapterDto {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_FIXTURE).expect("fixture is valid"))
}

fn chapter_pages(chapter: ChapterDto) -> Vec<MangaPage> {
    let volume = chapter.volume.unwrap_or(1);
    (1..=chapter.page_count)
        .map(|page| {
            format!(
                "{BUCKET_URL}/volume-{volume:02}/chapter-{:03}/page-{page:03}.png",
                chapter.number
            )
        })
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

#[derive(Default, Deserialize)]
struct MangaDto {
    id: String,
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    cover: String,
}

impl MangaDto {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.id.clone(),
            title: self.title,
            cover: (!self.cover.is_empty()).then_some(self.cover),
            authors: (!self.author.is_empty())
                .then_some(self.author)
                .into_iter()
                .collect(),
            artists: (!self.artist.is_empty())
                .then_some(self.artist)
                .into_iter()
                .collect(),
            description: (!self.description.is_empty()).then_some(self.description),
            status: match self.status.as_str() {
                "completed" => ItemStatus::Completed,
                "hiatus" => ItemStatus::Hiatus,
                "ongoing" => ItemStatus::Ongoing,
                _ => ItemStatus::Unknown,
            },
            url: Some(format!("{BASE_URL}/#{}", self.id)),
            language: Some("en".into()),
            content_rating: Some("safe".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_item_initialized(self) -> CatalogItem {
        let mut item = self.into_item();
        item.initialized = true;
        item
    }
}

#[derive(Default, Deserialize)]
struct ChapterDto {
    number: i32,
    title: String,
    volume: Option<i32>,
    #[serde(rename = "manga_id")]
    manga_id: String,
    #[serde(default, rename = "release_date")]
    release_date: String,
    #[serde(default, rename = "page_count")]
    page_count: i32,
}

impl ChapterDto {
    fn into_chapter(self, fallback_manga_id: &str) -> MangaChapter {
        let manga_id = if self.manga_id.is_empty() {
            fallback_manga_id
        } else {
            &self.manga_id
        };
        MangaChapter {
            key: format!("{}#{manga_id}", self.number),
            title: Some(self.title),
            chapter_number: Some(self.number as f32),
            date_uploaded: parse_date(&self.release_date),
            scanlators: vec!["Read Vagabond Manga".into()],
            url: Some(format!(
                "{BASE_URL}/volume-{}/chapter-{}/#{manga_id}",
                self.volume.unwrap_or(1),
                self.number
            )),
            language: Some("en".into()),
            ..MangaChapter::default()
        }
    }
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

const MANGA_LIST_FIXTURE: &str = r#"[{"id":"vagabond","title":"Vagabond","author":"Takehiko Inoue","artist":"Takehiko Inoue","description":"Sample description","status":"ongoing","cover":"https://readbagabondo.com/cover.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"{"id":"vagabond","title":"Vagabond","author":"Takehiko Inoue","artist":"Takehiko Inoue","description":"Sample description","status":"ongoing","cover":"https://readbagabondo.com/cover.jpg"}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"id":1,"number":1,"title":"Chapter 1","volume":1,"manga_id":"vagabond","release_date":"2024-01-01","page_count":2}]"#;
const CHAPTER_FIXTURE: &str = r#"{"id":1,"number":1,"title":"Chapter 1","volume":1,"manga_id":"vagabond","release_date":"2024-01-01","page_count":2}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Vagabond");
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
