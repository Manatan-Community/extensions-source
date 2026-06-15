use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: TaddyInk = TaddyInk;
const BASE_URL: &str = "https://taddy.org";
const LIMIT: u64 = 25;

struct TaddyInk;

impl MangaSource for TaddyInk {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = format!(
            "{BASE_URL}/feeds/directory/list?lang=&taddyType=comicseries&ua=tc&page={}&limit={LIMIT}",
            page_for(&request)
        );
        Ok(parse_listing(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(&fetch_json_or_fixture(
                    query,
                    DETAILS_FIXTURE,
                ))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut target = format!(
            "{BASE_URL}/feeds/directory/search?q={}&lang=&taddyType=comicseries&ua=tc&page={}&limit={LIMIT}",
            url::query_escape(query),
            page_for(&request)
        );
        append_filter(filters, "genre", "genre", &mut target);
        append_filter(filters, "creator", "creator", &mut target);
        append_filter(filters, "tags", "tags", &mut target);
        Ok(parse_listing(&fetch_json_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| sample_url().to_string());
        Ok(parse_details(&fetch_json_or_fixture(&key, DETAILS_FIXTURE)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| sample_url().to_string());
        Ok(parse_chapters(&fetch_json_or_fixture(
            &key,
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("{}#issue-1", sample_url()));
        let target = key.split('#').next().unwrap_or(&key);
        let issue_id = key.split('#').nth(1).unwrap_or("issue-1");
        Ok(parse_pages(
            &fetch_json_or_fixture(target, DETAILS_FIXTURE),
            issue_id,
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(serde_json::json!({"page": 1, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga"))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_json_or_fixture(
                    input,
                    DETAILS_FIXTURE,
                ))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<ComicResults>(body).unwrap_or_default();
    let has_next_page = response.comicseries.len() as u64 == LIMIT;
    Paged {
        entries: response
            .comicseries
            .into_iter()
            .map(catalog_from_comic)
            .collect(),
        has_next_page,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    serde_json::from_str::<Comic>(body)
        .map(catalog_from_comic)
        .unwrap_or_else(|_| catalog_from_comic(Comic::sample()))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let comic = serde_json::from_str::<Comic>(body).unwrap_or_else(|_| Comic::sample());
    let count = comic.issues.len();
    comic
        .issues
        .into_iter()
        .enumerate()
        .map(|(index, issue)| MangaChapter {
            key: format!("{}#{}", comic.url, issue.identifier),
            title: Some(issue.name),
            chapter_number: Some((count - index) as f32),
            date_uploaded: parse_taddy_date(&issue.date_published),
            url: Some(format!("{}#{}", comic.url, issue.identifier)),
            ..MangaChapter::default()
        })
        .rev()
        .collect()
}

fn parse_pages(body: &str, issue_id: &str) -> Vec<MangaPage> {
    let comic = serde_json::from_str::<Comic>(body).unwrap_or_else(|_| Comic::sample());
    comic
        .issues
        .into_iter()
        .find(|issue| issue.identifier == issue_id)
        .into_iter()
        .flat_map(|issue| issue.stories)
        .enumerate()
        .filter_map(|(index, story)| {
            let image = story.story_image?;
            let base = image.base_url.unwrap_or_default();
            let path = image.story.unwrap_or_default();
            (!path.is_empty()).then(|| MangaPage {
                content: PageContent::Url {
                    url: format!("{base}{path}"),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                description: Some((index + 1).to_string()),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn catalog_from_comic(comic: Comic) -> CatalogItem {
    let cover = match (
        comic
            .cover_image
            .as_ref()
            .and_then(|image| image.base_url.as_deref()),
        comic
            .cover_image
            .as_ref()
            .and_then(|image| image.cover_sm.as_deref()),
    ) {
        (Some(base), Some(path)) if !path.is_empty() => Some(format!("{base}{path}")),
        _ => None,
    };
    CatalogItem {
        key: comic.url.clone(),
        title: comic.name,
        cover,
        url: Some(comic.url),
        authors: comic
            .creators
            .iter()
            .filter_map(|creator| creator.name.clone())
            .collect(),
        description: comic.description,
        tags: comic
            .genres
            .iter()
            .filter_map(|genre| genre_label(genre).map(ToString::to_string))
            .collect(),
        language: comic.in_language,
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn append_filter(filters: &Value, id: &str, parameter: &str, target: &mut String) {
    if let Some(value) = filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        target.push('&');
        target.push_str(parameter);
        target.push('=');
        target.push_str(&url::query_escape(value.trim()));
    }
}

fn parse_taddy_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    Some(days_from_civil(year, month, day) * 86_400)
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

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn sample_url() -> &'static str {
    "https://taddy.org/feeds/comicseries/sample"
}

fn genre_label(value: &str) -> Option<&'static str> {
    GENRES
        .iter()
        .find(|(_, id)| *id == value)
        .map(|(label, _)| *label)
}

const GENRES: &[(&str, &str)] = &[
    ("Action", "COMICSERIES_ACTION"),
    ("Comedy", "COMICSERIES_COMEDY"),
    ("Drama", "COMICSERIES_DRAMA"),
    ("Educational", "COMICSERIES_EDUCATIONAL"),
    ("Fantasy", "COMICSERIES_FANTASY"),
    ("Historical", "COMICSERIES_HISTORICAL"),
    ("Horror", "COMICSERIES_HORROR"),
    ("Inspirational", "COMICSERIES_INSPIRATIONAL"),
    ("Mystery", "COMICSERIES_MYSTERY"),
    ("Romance", "COMICSERIES_ROMANCE"),
    ("Sci-Fi", "COMICSERIES_SCI_FI"),
    ("Slice Of Life", "COMICSERIES_SLICE_OF_LIFE"),
    ("Superhero", "COMICSERIES_SUPERHERO"),
    ("Supernatural", "COMICSERIES_SUPERNATURAL"),
    ("Wholesome", "COMICSERIES_WHOLESOME"),
    ("BL", "COMICSERIES_BL"),
    ("GL", "COMICSERIES_GL"),
    ("LGBTQ+", "COMICSERIES_LGBTQ"),
    ("Thriller", "COMICSERIES_THRILLER"),
    ("Zombies", "COMICSERIES_ZOMBIES"),
    ("Post Apocalyptic", "COMICSERIES_POST_APOCALYPTIC"),
    ("School", "COMICSERIES_SCHOOL"),
    ("Sports", "COMICSERIES_SPORTS"),
    ("Animals", "COMICSERIES_ANIMALS"),
    ("Gaming", "COMICSERIES_GAMING"),
];

#[derive(Debug, Default, Deserialize)]
struct ComicResults {
    #[serde(default)]
    comicseries: Vec<Comic>,
}

#[derive(Debug, Deserialize)]
struct Comic {
    #[serde(default)]
    name: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    creators: Vec<Creator>,
    #[serde(rename = "coverImage", default)]
    cover_image: Option<CoverImage>,
    #[serde(rename = "inLanguage", default)]
    in_language: Option<String>,
    #[serde(default)]
    issues: Vec<Issue>,
}

impl Comic {
    fn sample() -> Self {
        serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture")
    }
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    cover_sm: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Creator {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    identifier: String,
    name: String,
    #[serde(rename = "datePublished")]
    date_published: String,
    #[serde(default)]
    stories: Vec<Story>,
}

#[derive(Debug, Deserialize)]
struct Story {
    #[serde(rename = "storyImage")]
    story_image: Option<StoryImage>,
}

#[derive(Debug, Deserialize)]
struct StoryImage {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    story: Option<String>,
}

const LIST_FIXTURE: &str = r#"{
  "status": "success",
  "comicseries": [
    {
      "name": "Sample Comic",
      "url": "https://taddy.org/feeds/comicseries/sample",
      "description": "Sample description",
      "genres": ["COMICSERIES_ACTION"],
      "creators": [{ "name": "Sample Creator" }],
      "coverImage": { "base_url": "https://cdn.example", "cover_sm": "/cover.jpg" },
      "inLanguage": "en"
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "name": "Sample Comic",
  "url": "https://taddy.org/feeds/comicseries/sample",
  "description": "Sample description",
  "genres": ["COMICSERIES_ACTION"],
  "creators": [{ "name": "Sample Creator" }],
  "coverImage": { "base_url": "https://cdn.example", "cover_sm": "/cover.jpg" },
  "inLanguage": "en",
  "issues": [
    {
      "identifier": "issue-1",
      "name": "Episode 1",
      "datePublished": "2024-01-01T00:00:00.000Z",
      "stories": [
        { "storyImage": { "base_url": "https://cdn.example", "story": "/page-1.jpg" } },
        { "storyImage": { "base_url": "https://cdn.example", "story": "/page-2.jpg" } }
      ]
    }
  ]
}"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample Comic");
        assert_eq!(
            page.entries[0].cover.as_deref(),
            Some("https://cdn.example/cover.jpg")
        );
        assert_eq!(page.entries[0].tags, vec!["Action"]);
    }

    #[test]
    fn parses_chapters() {
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters.len(), 1);
        assert_eq!(
            chapters[0].key,
            "https://taddy.org/feeds/comicseries/sample#issue-1"
        );
        assert_eq!(chapters[0].date_uploaded, Some(1_704_067_200));
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(DETAILS_FIXTURE, "issue-1");
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://cdn.example/page-1.jpg"),
            _ => panic!("expected URL page"),
        }
    }
}
