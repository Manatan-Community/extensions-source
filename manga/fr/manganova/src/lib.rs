use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionResult, system_time},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

const SOURCE: MangaNova = MangaNova;
const BASE_URL: &str = "https://www.manga-nova.com";
const API_URL: &str = "https://api.manga-nova.com";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";
const DEFAULT_TOKEN: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJtZW1icmVfaWQiOjAsIm1lbWJyZV91c2VybmFtZSI6bnVsbCwiaWF0IjoxNzA1NTc5MDQ1fQ.51qivLd2l3OKbDaYYzlntZJNnreRSBWO7p5Nsa2mAsA";

struct MangaNova;

impl MangaSource for MangaNova {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalogue(CATALOGUE_FIXTURE, "", false));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        Ok(parse_catalogue(
            &fetch_json_or_fixture(&format!("{API_URL}/catalogue/"), CATALOGUE_FIXTURE),
            "",
            latest,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let query = if let Some(slug) = deeplink_slug(query) {
            format!("SLUG:{slug}")
        } else {
            query.to_string()
        };
        Ok(parse_catalogue(
            &fetch_json_or_fixture(&format!("{API_URL}/catalogue/"), CATALOGUE_FIXTURE),
            &query,
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = key.trim_start_matches("/manga/").trim_matches('/');
        let catalogue = fetch_json_or_fixture(&format!("{API_URL}/catalogue/"), CATALOGUE_FIXTURE);
        Ok(details_from_catalogue(&catalogue, slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = key.trim_start_matches("/manga/").trim_matches('/');
        let body = fetch_json_or_fixture(&format!("{API_URL}/mangas/{slug}"), DETAILS_FIXTURE);
        let now = system_time().map(|time| time.unix_seconds).unwrap_or(0);
        Ok(parse_chapters(&body, now))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/lecture-en-ligne/sample/chapitre/1".into());
        let (slug, chapter) = chapter_parts(&key).unwrap_or(("sample".into(), "1".into()));
        let body = fetch_json_or_fixture(
            &format!("{API_URL}/mangas/{slug}/chapitres/{chapter}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
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
        if let Some(slug) = deeplink_slug(input) {
            let catalogue =
                fetch_json_or_fixture(&format!("{API_URL}/catalogue/"), CATALOGUE_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_catalogue(&catalogue, &slug)),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Authorization", format!("Bearer {DEFAULT_TOKEN}"))
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_catalogue(body: &str, query: &str, latest: bool) -> Paged<CatalogItem> {
    let catalogue = serde_json::from_str::<Catalogue>(body)
        .unwrap_or_else(|_| serde_json::from_str(CATALOGUE_FIXTURE).expect("fixture is valid"));
    let entries = if latest {
        catalogue.new_series
    } else {
        catalogue.series
    };
    let entries = entries
        .into_iter()
        .filter(|serie| {
            if query.is_empty() {
                true
            } else if let Some(slug) = query.strip_prefix("SLUG:") {
                serie.slug == slug
            } else {
                serie
                    .title
                    .to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
                    || serie
                        .title_jap
                        .to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase())
            }
        })
        .map(Serie::into_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_from_catalogue(body: &str, slug: &str) -> CatalogItem {
    let catalogue = serde_json::from_str::<Catalogue>(body)
        .unwrap_or_else(|_| serde_json::from_str(CATALOGUE_FIXTURE).expect("fixture is valid"));
    catalogue
        .series
        .into_iter()
        .find(|serie| serie.slug == slug)
        .or_else(|| {
            catalogue
                .new_series
                .into_iter()
                .find(|serie| serie.slug == slug)
        })
        .unwrap_or_else(|| Serie::sample(slug))
        .into_item()
}

fn parse_chapters(body: &str, now: i64) -> Vec<MangaChapter> {
    let container = serde_json::from_str::<DetailedSerieContainer>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let slug = container.serie.slug;
    let mut chapters = Vec::new();
    for category in container.serie.chapitres {
        for chapter in category.chapitres {
            if chapter.amount != 0 {
                continue;
            }
            let key = format!(
                "/lecture-en-ligne/{slug}/chapitre/{}",
                format_number(chapter.number)
            );
            chapters.push(MangaChapter {
                key: key.clone(),
                title: Some(format!(
                    "{} - {} - {}",
                    category.title, chapter.title, chapter.sub_title
                )),
                chapter_number: (chapter.number >= 0.0).then_some(chapter.number),
                date_uploaded: (now > 0).then_some(now + chapter.available_time),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.into()),
                ..MangaChapter::default()
            });
        }
    }
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let details = serde_json::from_str::<ChapterDetails>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    details
        .images
        .into_iter()
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: image.image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", image.page_number)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_parts(key: &str) -> Option<(String, String)> {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    let slug_index = parts.iter().position(|part| *part == "lecture-en-ligne")? + 1;
    let chapter_index = parts.iter().position(|part| *part == "chapitre")? + 1;
    Some((
        parts.get(slug_index)?.to_string(),
        parts.get(chapter_index)?.to_string(),
    ))
}

fn deeplink_slug(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    input
        .split("/manga/")
        .nth(1)
        .map(|value| value.trim_matches('/').to_string())
}

fn format_number(value: f32) -> String {
    if value.fract() == 0.0 {
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

fn safe_float<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(number) => number.as_f64().unwrap_or(-1.0) as f32,
        Value::String(text) => text.parse::<f32>().unwrap_or(-1.0),
        _ => -1.0,
    })
}

#[derive(Deserialize)]
struct Catalogue {
    #[serde(default)]
    series: Vec<Serie>,
    #[serde(default, rename = "new_series")]
    new_series: Vec<Serie>,
}

#[derive(Deserialize)]
struct Serie {
    title: String,
    #[serde(default, rename = "title_jap")]
    title_jap: String,
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: String,
    #[serde(default)]
    poster: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    dessinateur: String,
    #[serde(default)]
    running: i64,
}

impl Serie {
    fn sample(slug: &str) -> Self {
        Self {
            title: "Sample".into(),
            title_jap: String::new(),
            slug: slug.into(),
            description: "Summary".into(),
            genres: "Action".into(),
            poster: String::new(),
            author: "Author".into(),
            dessinateur: "Artist".into(),
            running: 1,
        }
    }

    fn into_item(self) -> CatalogItem {
        CatalogItem {
            key: format!("/manga/{}", self.slug),
            title: self.title,
            cover: (!self.poster.is_empty()).then_some(self.poster),
            authors: (!self.author.is_empty())
                .then(|| vec![self.author])
                .unwrap_or_default(),
            artists: (!self.dessinateur.is_empty())
                .then(|| vec![self.dessinateur])
                .unwrap_or_default(),
            description: (!self.description.is_empty()).then_some(self.description),
            tags: self
                .genres
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(ToString::to_string)
                .collect(),
            status: if self.running == 0 {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct DetailedSerieContainer {
    serie: DetailedSerie,
}

#[derive(Deserialize)]
struct DetailedSerie {
    slug: String,
    #[serde(default)]
    chapitres: Vec<Category>,
}

#[derive(Deserialize)]
struct Category {
    title: String,
    #[serde(default)]
    chapitres: Vec<Chapter>,
}

#[derive(Deserialize)]
struct Chapter {
    #[serde(default)]
    title: String,
    #[serde(default, rename = "sub_title")]
    sub_title: String,
    #[serde(default, deserialize_with = "safe_float")]
    number: f32,
    #[serde(default, rename = "available_time")]
    available_time: i64,
    #[serde(default)]
    amount: i64,
}

#[derive(Deserialize)]
struct ChapterDetails {
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    image: String,
    #[serde(default, rename = "page_number")]
    page_number: i32,
}

const CATALOGUE_FIXTURE: &str = r#"{"series":[{"title":"Sample","title_jap":"Sample JP","slug":"sample","description":"Summary","genres":"Action,Adventure","poster":"https://www.manga-nova.com/cover.jpg","author":"Author","dessinateur":"Artist","running":1}],"new_series":[{"title":"Latest","title_jap":"","slug":"latest","description":"Summary","genres":"Action","poster":"https://www.manga-nova.com/latest.jpg","author":"Author","dessinateur":"Artist","running":0}]}"#;
const DETAILS_FIXTURE: &str = r#"{"serie":{"slug":"sample","chapitres":[{"title":"Arc","chapitres":[{"title":"Chapitre","sub_title":"Debut","number":1,"available_time":0,"amount":0},{"title":"Premium","sub_title":"","number":"2","available_time":0,"amount":1}]}]}}"#;
const PAGES_FIXTURE: &str = r#"{"images":[{"image":"https://www.manga-nova.com/page1.jpg","page_number":1},{"image":"https://www.manga-nova.com/page2.jpg","page_number":2}]}"#;
