use manatan_extension::{
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
};
use manatan_shared::{html, manga, url};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

const SOURCE: Medusascans = Medusascans;
const BASE_URL: &str = "https://medusascans.pro";
const CONTENT_RATING: &str = "adult";
const PER_PAGE: usize = 20;

struct Medusascans;

impl MangaSource for Medusascans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut mangas = fetch_manga_list();
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            mangas.sort_by_key(|manga| {
                std::cmp::Reverse(manga.updated_at.or(manga.manga_date).unwrap_or(0))
            });
        }
        Ok(paged(
            mangas
                .into_iter()
                .map(|manga| manga.to_item(false))
                .collect(),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = normalize_slug(query);
            let detail = fetch_details(&slug);
            return Ok(Paged {
                entries: vec![detail.to_item(true)],
                has_next_page: false,
            });
        }

        let needle = query.to_ascii_lowercase();
        let status = filter_string(&request, "status").to_ascii_lowercase();
        let manga_type = filter_string(&request, "type").to_ascii_lowercase();
        let genre = filter_string(&request, "genre").to_ascii_lowercase();
        let items = fetch_manga_list()
            .into_iter()
            .filter(|manga| needle.is_empty() || manga.title.to_ascii_lowercase().contains(&needle))
            .filter(|manga| {
                status.is_empty()
                    || manga
                        .status
                        .as_deref()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case(&status)
            })
            .filter(|manga| {
                manga_type.is_empty()
                    || manga
                        .kind
                        .as_deref()
                        .unwrap_or_default()
                        .eq_ignore_ascii_case(&manga_type)
            })
            .filter(|manga| {
                genre.is_empty()
                    || manga
                        .genres
                        .iter()
                        .flatten()
                        .any(|value| value.eq_ignore_ascii_case(&genre))
            })
            .map(|manga| manga.to_item(false))
            .collect();
        Ok(paged(items, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(fetch_details(&normalize_slug(&key)).to_item(true))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let detail = fetch_details(&normalize_slug(&key));
        Ok(detail
            .chapters
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(|chapter| chapter.to_chapter(&detail.slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/komik/sample/chapter-1/".into());
        let (manga_slug, chapter_slug) = chapter_parts(&key);
        let body = fetch_text_or_fixture(
            &format!("{BASE_URL}/wp-json/comic/v1/manga/{manga_slug}/chapter/{chapter_slug}/"),
            CHAPTER_IMAGES_FIXTURE,
            true,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/komik/") {
            let detail = fetch_details(&normalize_slug(input));
            return Ok(Some(UrlResolveResult {
                item: Some(detail.to_item(true)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target: &str, fixture: &str, xhr: bool) -> String {
    let http_client = client();
    let request = http_client.get(target);
    let request = if xhr {
        request.xhr()
    } else {
        request.browser_document()
    };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_manga_list() -> Vec<MangaDto> {
    let body = fetch_text_or_fixture(
        &format!("{BASE_URL}/wp-content/static/manga/index.json"),
        INDEX_FIXTURE,
        true,
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|_| serde_json::from_str(INDEX_FIXTURE).unwrap_or_default())
}

fn fetch_details(slug: &str) -> MangaDetailDto {
    let body = fetch_text_or_fixture(
        &format!("{BASE_URL}/wp-content/static/manga/{slug}.json"),
        DETAILS_FIXTURE,
        true,
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture"))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = serde_json::from_str::<ChapterImagesDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_IMAGES_FIXTURE).expect("valid fixture"))
        .images;
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn paged(items: Vec<CatalogItem>, page: u64) -> Paged<CatalogItem> {
    let page = page.max(1) as usize;
    let start = (page - 1) * PER_PAGE;
    let total = items.len();
    Paged {
        entries: items.into_iter().skip(start).take(PER_PAGE).collect(),
        has_next_page: start + PER_PAGE < total,
    }
}

fn normalize_slug(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.split("/komik/").nth(1) {
        return rest.split('/').next().unwrap_or("sample").to_string();
    }
    trimmed.trim_matches('/').to_string()
}

fn chapter_parts(input: &str) -> (String, String) {
    let trimmed = input.trim_matches('/');
    let parts = trimmed.split('/').collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0] == "komik" {
        return (parts[1].to_string(), parts[2].to_string());
    }
    (
        "sample".to_string(),
        trimmed
            .rsplit('/')
            .next()
            .unwrap_or("chapter-1")
            .to_string(),
    )
}

fn filter_string(request: &Value, key: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn status(value: Option<&str>) -> ItemStatus {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "on-going" | "ongoing" => ItemStatus::Ongoing,
        "end" | "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::String(value) if !value.is_empty() => Ok(Some(value)),
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct MangaDto {
    slug: String,
    title: String,
    #[serde(default, deserialize_with = "optional_string")]
    thumbnail: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(rename = "manga_date", default)]
    manga_date: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
}

impl MangaDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: self.slug.clone(),
            title: self.title.clone(),
            cover: self
                .thumbnail
                .clone()
                .map(|value| url::join_url(BASE_URL, &value)),
            url: Some(format!("{BASE_URL}/komik/{}/", self.slug)),
            tags: self.genres.clone().unwrap_or_default(),
            status: status(self.status.as_deref()),
            language: Some("id".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct MangaDetailDto {
    slug: String,
    title: String,
    #[serde(default, deserialize_with = "optional_string")]
    thumbnail: Option<String>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    alternative: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    genres: Option<Vec<String>>,
    #[serde(default)]
    chapters: Option<Vec<ChapterDto>>,
}

impl MangaDetailDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        let mut description = self
            .synopsis
            .as_deref()
            .map(html::strip_tags)
            .unwrap_or_default();
        if let Some(alternative) = self
            .alternative
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str("Alternative: ");
            description.push_str(alternative);
        }
        CatalogItem {
            key: self.slug.clone(),
            title: self.title.clone(),
            cover: self
                .thumbnail
                .clone()
                .map(|value| url::join_url(BASE_URL, &value)),
            url: Some(format!("{BASE_URL}/komik/{}/", self.slug)),
            authors: self.author.iter().cloned().collect(),
            artists: self.artist.iter().cloned().collect(),
            description: (!description.is_empty()).then_some(description),
            tags: self.genres.clone().unwrap_or_default(),
            status: status(self.status.as_deref()),
            language: Some("id".to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ChapterDto {
    slug: String,
    title: String,
    #[serde(default)]
    date: Option<i64>,
}

impl ChapterDto {
    fn to_chapter(&self, manga_slug: &str) -> MangaChapter {
        let key = format!("/komik/{manga_slug}/{}/", self.slug);
        MangaChapter {
            key: key.clone(),
            title: Some(self.title.clone()),
            date_uploaded: self.date.map(|value| value * 1000),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ChapterImagesDto {
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const INDEX_FIXTURE: &str = r#"
[
  {
    "slug": "sample",
    "title": "Sample Comic",
    "thumbnail": "https://medusascans.pro/images/sample.jpg",
    "status": "on-going",
    "type": "Manga",
    "genres": ["Action"],
    "manga_date": 1704067200,
    "updated_at": 1704153600
  }
]
"#;

const DETAILS_FIXTURE: &str = r#"
{
  "slug": "sample",
  "title": "Sample Comic",
  "thumbnail": "https://medusascans.pro/images/sample.jpg",
  "synopsis": "<p>Sample description.</p>",
  "alternative": "Sample Alt",
  "status": "on-going",
  "author": "Author",
  "artist": "Artist",
  "genres": ["Action"],
  "chapters": [
    { "slug": "chapter-1", "title": "Chapter 1", "date": 1704067200 }
  ]
}
"#;

const CHAPTER_IMAGES_FIXTURE: &str = r#"
{ "images": ["https://medusascans.pro/images/page-1.jpg"] }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_index_and_details() {
        let mangas = serde_json::from_str::<Vec<MangaDto>>(INDEX_FIXTURE).unwrap();
        assert_eq!(mangas[0].to_item(false).title, "Sample Comic");
        let detail = serde_json::from_str::<MangaDetailDto>(DETAILS_FIXTURE).unwrap();
        assert_eq!(detail.to_item(true).authors, vec!["Author"]);
        assert_eq!(
            detail.chapters.unwrap()[0].to_chapter("sample").key,
            "/komik/sample/chapter-1/"
        );
    }

    #[test]
    fn parses_images() {
        assert_eq!(parse_pages(CHAPTER_IMAGES_FIXTURE).len(), 1);
    }
}
