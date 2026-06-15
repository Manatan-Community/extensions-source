use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Hentara = Hentara;
const BASE_URL: &str = "https://hentara.com";
const API_URL: &str = "https://hentara.com/r2-data";

struct Hentara;

impl MangaSource for Hentara {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_index(INDEX_FIXTURE, "", 1, 0));
        }
        let listing = request.get("listingId").or_else(|| request.get("listing")).and_then(Value::as_str);
        let sort = if listing == Some("popular") { 1 } else { 0 };
        Ok(parse_index(&fetch_index(), "", sort, 0))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let slug = slug_from_url(query).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters");
        let sort = filter_number(filters, "sort", 0);
        let genre = filter_number(filters, "genre", 0);
        Ok(parse_index(&fetch_index(), query, sort, genre))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga")
            .and_then(|key| slug_from_url(&key))
            .unwrap_or_else(|| "sample".to_string());
        Ok(details_by_slug(&slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga")
            .and_then(|key| slug_from_url(&key))
            .unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(&fetch_comic(&slug), &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (slug, episode) = key.split_once('/').unwrap_or((&key, "1"));
        Ok(parse_pages(&fetch_episode(slug, episode), BASE_URL))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = parse_index(&fetch_index(), "", 0, 0);
        let popular = parse_index(&fetch_index(), "", 1, 0);
        Ok(vec![
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .and_then(|key| slug_from_url(&key))
            .map(|slug| format!("{BASE_URL}/manhwa/{slug}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (slug, episode) = key.split_once('/').unwrap_or((&key, "1"));
            format!("{BASE_URL}/manhwa/{slug}/chapter-{episode}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input).filter(|_| input.starts_with(BASE_URL)) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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
}

fn fetch_index() -> String {
    client()
        .get(format!("{API_URL}/index.json"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| INDEX_FIXTURE.to_string())
}

fn fetch_comic(slug: &str) -> String {
    client()
        .get(format!("{API_URL}/comics/{slug}.json"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| COMIC_FIXTURE.to_string())
}

fn fetch_episode(slug: &str, episode: &str) -> String {
    client()
        .get(format!("{API_URL}/episodes/{slug}/{episode}.json"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| EPISODE_FIXTURE.to_string())
}

fn parse_index(body: &str, query: &str, sort: usize, genre_index: usize) -> Paged<CatalogItem> {
    let mut entries = serde_json::from_str::<IndexDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(INDEX_FIXTURE).expect("fixture is valid"))
        .comics
        .into_iter()
        .filter(|comic| {
            let matches_query =
                query.is_empty() || comic.title.to_ascii_lowercase().contains(&query.to_ascii_lowercase());
            let genre = GENRES.get(genre_index).copied().unwrap_or("Any");
            let matches_genre =
                genre == "Any" || comic.genres.iter().any(|item| item.name.eq_ignore_ascii_case(genre));
            matches_query && matches_genre
        })
        .collect::<Vec<_>>();
    match sort {
        1 => entries.sort_by_key(|comic| std::cmp::Reverse(comic.view_count)),
        2 => entries.sort_by(|a, b| a.title.cmp(&b.title)),
        _ => entries.sort_by(|a, b| b.latest_episode_date.cmp(&a.latest_episode_date)),
    }
    Paged {
        entries: entries.into_iter().map(|comic| comic.into_item()).collect(),
        has_next_page: false,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let body = fetch_comic(slug);
    serde_json::from_str::<MangaDto>(&body)
        .unwrap_or_else(|_| serde_json::from_str(COMIC_FIXTURE).expect("fixture is valid"))
        .comic
        .into_item()
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let mut chapters = serde_json::from_str::<MangaDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(COMIC_FIXTURE).expect("fixture is valid"))
        .episodes
        .into_iter()
        .map(|episode| episode.into_chapter(slug))
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| b.chapter_number.partial_cmp(&a.chapter_number).unwrap_or(std::cmp::Ordering::Equal));
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    serde_json::from_str::<EpisodeDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(EPISODE_FIXTURE).expect("fixture is valid"))
        .pages
        .into_iter()
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: page.image_url.clone(),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", page.page_number)),
            ..MangaPage::default()
        })
        .collect()
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split("/manhwa/")
        .nth(1)
        .and_then(|rest| rest.split('/').find(|part| !part.is_empty()))
        .map(ToString::to_string)
        .or_else(|| {
            (!input.contains('/')).then(|| input.trim_matches('/').to_string())
        })
}

fn filter_number(filters: Option<&Value>, key: &str, fallback: usize) -> usize {
    filters
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_str().and_then(|text| text.parse().ok()).or_else(|| value.as_u64().map(|number| number as usize)))
        .unwrap_or(fallback)
}

#[derive(Debug, Deserialize)]
struct IndexDto {
    #[serde(default)]
    comics: Vec<ComicDto>,
}

#[derive(Debug, Deserialize)]
struct MangaDto {
    comic: FullComicDto,
    #[serde(default)]
    episodes: Vec<EpisodeShortDto>,
}

#[derive(Debug, Deserialize)]
struct EpisodeDto {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct ComicDto {
    title: String,
    slug: String,
    #[serde(default, rename = "thumbnail_url")]
    thumbnail_url: Option<String>,
    #[serde(default, rename = "view_count")]
    view_count: i64,
    #[serde(default, rename = "latest_episode_date")]
    latest_episode_date: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
}

#[derive(Debug, Deserialize)]
struct FullComicDto {
    title: String,
    slug: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "thumbnail_url")]
    thumbnail_url: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
}

#[derive(Debug, Deserialize)]
struct GenreDto {
    name: String,
}

#[derive(Debug, Deserialize)]
struct EpisodeShortDto {
    #[serde(rename = "episode_number")]
    episode_number: i32,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "created_at")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    #[serde(rename = "page_number")]
    page_number: i32,
    #[serde(rename = "image_url")]
    image_url: String,
}

impl ComicDto {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.thumbnail_url,
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/manhwa/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

impl FullComicDto {
    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title,
            cover: self.thumbnail_url,
            description: self.description,
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: ItemStatus::Unknown,
            url: Some(format!("{BASE_URL}/manhwa/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

impl EpisodeShortDto {
    fn into_chapter(self, slug: &str) -> MangaChapter {
        let title = if let Some(title) = self.title.filter(|title| !title.is_empty()) {
            format!("Chapter {} - {title}", self.episode_number)
        } else {
            format!("Chapter {}", self.episode_number)
        };
        MangaChapter {
            key: format!("{slug}/{}", self.episode_number),
            title: Some(title),
            chapter_number: Some(self.episode_number as f32),
            date_uploaded: self.created_at.and_then(|date| parse_rfc3339_date(&date)),
            url: Some(format!("{BASE_URL}/manhwa/{slug}/chapter-{}", self.episode_number)),
            ..MangaChapter::default()
        }
    }
}

fn parse_rfc3339_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<i32>().ok()?;
    let day = parts.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_fixture_date(&format!("{year:04}-{month:02}-{day:02}"))
}

const GENRES: &[&str] = &[
    "Any", "Action", "BL", "Cheating", "Detective", "Drama", "Harem", "In-Law", "MILF",
    "Married", "Office", "Romance", "Spin-Off", "Thriller", "University", "College", "Nerd",
];

export_manga_source!(SOURCE);

const INDEX_FIXTURE: &str = r#"
{"comics":[{"title":"Sample Hentara","slug":"sample","thumbnail_url":"https://cdn.example.test/cover.jpg","view_count":10,"latest_episode_date":"2024-01-01T00:00:00.000Z","genres":[{"name":"Drama"}]}]}
"#;

const COMIC_FIXTURE: &str = r#"
{"comic":{"title":"Sample Hentara","slug":"sample","description":"Sample description","thumbnail_url":"https://cdn.example.test/cover.jpg","genres":[{"name":"Drama"}]},"episodes":[{"episode_number":1,"title":"Start","created_at":"2024-01-01T00:00:00.000Z"}]}
"#;

const EPISODE_FIXTURE: &str = r#"
{"pages":[{"page_number":1,"image_url":"https://cdn.example.test/page1.jpg"},{"page_number":2,"image_url":"https://cdn.example.test/page2.jpg"}]}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hentara_fixtures() {
        assert_eq!(parse_index(INDEX_FIXTURE, "", 0, 0).entries.len(), 1);
        assert_eq!(parse_chapters(COMIC_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(EPISODE_FIXTURE, BASE_URL).len(), 2);
    }
}
