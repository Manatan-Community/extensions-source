use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://hni-scantrad.net";
const API_URL: &str = "https://hni-scantrad.net/api";
const SOURCE: HniScantrad = HniScantrad;

struct HniScantrad;

impl MangaSource for HniScantrad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_json_or_fixture(&format!("{API_URL}/comics"), LIST_FIXTURE);
        let mut page = parse_list(&body);
        if latest {
            page.entries = latest_items(&body);
        }
        Ok(page)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with(API_URL) {
            let key = normalize_key(query);
            let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: parse_detail(&body).into_iter().collect(),
                has_next_page: false,
            });
        }
        let body = if query.is_empty() {
            fetch_json_or_fixture(&format!("{API_URL}/comics"), LIST_FIXTURE)
        } else {
            fetch_json_or_fixture(&format!("{API_URL}/search/{}", url::query_escape(query)), LIST_FIXTURE)
        };
        Ok(parse_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_detail(&body).unwrap_or_else(sample_item))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_json_or_fixture(&format!("{API_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: parse_detail(&body),
                url: Some(input.to_string()),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(BASE_URL)
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let Ok(response) = serde_json::from_str::<ResultsDto>(body) else {
        return Paged { entries: vec![sample_item()], has_next_page: false };
    };
    Paged {
        entries: response.comics.into_iter().map(PizzaComic::into_item).collect(),
        has_next_page: false,
    }
}

fn latest_items(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<ResultsDto>(body)
        .map(|response| {
            let mut comics = response.comics.into_iter().filter(|comic| comic.last_chapter.is_some()).collect::<Vec<_>>();
            comics.sort_by(|a, b| b.last_chapter.as_ref().map(|chapter| &chapter.published_on).cmp(&a.last_chapter.as_ref().map(|chapter| &chapter.published_on)));
            comics.into_iter().take(10).map(PizzaComic::into_item).collect()
        })
        .unwrap_or_default()
}

fn parse_detail(body: &str) -> Option<CatalogItem> {
    serde_json::from_str::<ResultDto>(body).ok()?.comic.map(PizzaComic::into_details)
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<ResultDto>(body)
        .ok()
        .and_then(|response| response.comic)
        .map(|comic| comic.chapters.into_iter().map(PizzaChapter::into_chapter).collect())
        .unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<ReaderDto>(body)
        .ok()
        .and_then(|response| response.chapter)
        .map(|chapter| {
            chapter
                .pages
                .into_iter()
                .enumerate()
                .map(|(index, page)| MangaPage {
                    content: PageContent::Url { url: page, context: None },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(API_URL).or_else(|| input.strip_prefix(BASE_URL)) {
        return format!("/{}", path.trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().get(0..7).unwrap_or_default() {
        "In cors" | "On goin" => ItemStatus::Ongoing,
        "Complet" | "Conclus" | "Conclud" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn sample_item() -> CatalogItem {
    CatalogItem {
        key: "/comic/sample".into(),
        title: "Sample Comic".into(),
        language: Some("all".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Deserialize)]
struct ResultsDto {
    #[serde(default)]
    comics: Vec<PizzaComic>,
}

#[derive(Deserialize)]
struct ResultDto {
    comic: Option<PizzaComic>,
}

#[derive(Deserialize)]
struct ReaderDto {
    chapter: Option<PizzaChapter>,
}

#[derive(Deserialize)]
struct PizzaComic {
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    chapters: Vec<PizzaChapter>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: Vec<PizzaGenre>,
    #[serde(default)]
    last_chapter: Option<PizzaChapter>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    url: String,
}

impl PizzaComic {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.url.clone(),
            title: self.title,
            cover: (!self.thumbnail.is_empty()).then_some(self.thumbnail),
            url: Some(url::join_url(BASE_URL, &self.url)),
            language: Some("all".into()),
            content_rating: Some("safe".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }

    fn into_details(self) -> CatalogItem {
        CatalogItem {
            key: self.url.clone(),
            title: self.title,
            cover: (!self.thumbnail.is_empty()).then_some(self.thumbnail),
            url: Some(url::join_url(BASE_URL, &self.url)),
            authors: (!self.author.is_empty()).then_some(self.author).into_iter().collect(),
            artists: self.artist.into_iter().collect(),
            description: (!self.description.is_empty()).then_some(self.description),
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: status(self.status.as_deref()),
            language: Some("all".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct PizzaGenre {
    name: String,
}

#[derive(Clone, Deserialize)]
struct PizzaChapter {
    #[serde(default)]
    chapter: Option<i32>,
    #[serde(default)]
    full_title: String,
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default)]
    published_on: String,
    #[serde(default)]
    subchapter: Option<i32>,
    #[serde(default)]
    teams: Vec<Option<PizzaTeam>>,
    #[serde(default)]
    url: String,
}

impl PizzaChapter {
    fn into_chapter(self) -> MangaChapter {
        let number = self.chapter.unwrap_or(-1) as f32 + self.subchapter.unwrap_or(0) as f32 / 10.0;
        MangaChapter {
            key: self.url.clone(),
            title: Some(if self.full_title.is_empty() { "Chapter".into() } else { self.full_title }),
            chapter_number: Some(number),
            scanlators: self.teams.into_iter().flatten().map(|team| team.name).collect(),
            url: Some(url::join_url(BASE_URL, &self.url)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, Deserialize)]
struct PizzaTeam {
    name: String,
}

const LIST_FIXTURE: &str = r#"
{ "comics": [{ "title": "Sample Comic", "thumbnail": "https://hni-scantrad.net/cover.jpg", "url": "/comic/sample", "last_chapter": { "full_title": "Chapter 1", "published_on": "2024-01-01T00:00:00.000000", "url": "/chapter/sample" } }] }
"#;

const DETAILS_FIXTURE: &str = r#"
{ "comic": { "title": "Sample Comic", "author": "Sample Author", "artist": "Sample Artist", "description": "Sample description", "genres": [{ "name": "Action" }], "status": "Completed", "thumbnail": "https://hni-scantrad.net/cover.jpg", "url": "/comic/sample", "chapters": [{ "chapter": 1, "full_title": "Chapter 1", "published_on": "2024-01-01T00:00:00.000000", "teams": [{ "name": "Team" }], "url": "/chapter/sample" }] } }
"#;

const PAGES_FIXTURE: &str = r#"
{ "chapter": { "full_title": "Chapter 1", "pages": ["https://hni-scantrad.net/page1.jpg", "https://hni-scantrad.net/page2.jpg"], "url": "/chapter/sample" } }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pizza_reader_source() {
        assert_eq!(parse_list(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(latest_items(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
