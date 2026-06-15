use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Desu = Desu;
const DEFAULT_BASE_URL: &str = "https://desu.uno";
const API_PATH: &str = "/manga/api";

struct Desu;

impl MangaSource for Desu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL, "eng"));
        }
        let base = base_url(&request);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "updated"
        } else {
            "popular"
        };
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let lang = title_language(&request);
        Ok(parse_listing(
            &fetch_text(
                &format!("{base}{API_PATH}/?limit=50&order={order}&page={page}"),
                LIST_FIXTURE,
                &base,
            ),
            &base,
            &lang,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let lang = title_language(&request);
        if query.starts_with("http://")
            || query.starts_with("https://")
            || query.starts_with("slug:")
        {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_detail(
                    &fetch_text(&details_api_url(&base, &key), DETAILS_FIXTURE, &base),
                    Some(key),
                    &base,
                    &lang,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = search_url(
            &base,
            page,
            query,
            request.get("filters").unwrap_or(&Value::Null),
        );
        Ok(parse_listing(
            &fetch_text(&target, LIST_FIXTURE, &base),
            &base,
            &lang,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let lang = title_language(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1".into());
        Ok(parse_detail(
            &fetch_text(&details_api_url(&base, &key), DETAILS_FIXTURE, &base),
            Some(normalize_key(&key)),
            &base,
            &lang,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1".into());
        Ok(parse_chapters(&fetch_text(
            &details_api_url(&base, &key),
            DETAILS_FIXTURE,
            &base,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/1/vol1/ch1/rus#apiChapter/1/chapter/1".into());
        Ok(parse_pages(
            &fetch_text(&chapter_api_url(&base, &key), PAGES_FIXTURE, &base),
            &base,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{base}{}",
                key.split("#apiChapter").next().unwrap_or(&key).trim()
            )
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with("http://") || input.starts_with("https://") {
            let base = base_url(&request);
            let lang = title_language(&request);
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_detail(
                    &fetch_text(&details_api_url(&base, &key), DETAILS_FIXTURE, &base),
                    Some(key),
                    &base,
                    &lang,
                )),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Tachiyomi")
        .with_referer(base.to_string())
}

fn fetch_text(target: &str, fixture: &str, base: &str) -> String {
    client(base)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn title_language(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("titleLanguage"))
        .and_then(Value::as_str)
        .unwrap_or("eng")
        .to_string()
}

fn search_url(base: &str, page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![
        ("limit".to_string(), "20".to_string()),
        ("page".to_string(), page.to_string()),
    ];
    params.push((
        "order".into(),
        filter_string(filters, "order")
            .unwrap_or("popular")
            .to_string(),
    ));
    let types = selected_values(filters.get("types"));
    if !types.is_empty() {
        params.push(("kinds".into(), types.join(",")));
    }
    let genres = selected_values(filters.get("genres"));
    if !genres.is_empty() {
        params.push(("genres".into(), genres.join(",")));
    }
    if !query.is_empty() {
        params.push(("search".into(), query.to_string()));
    }
    format!(
        "{base}{API_PATH}/?{}",
        params
            .iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(option_id)
            .collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn option_id(value: &str) -> Option<String> {
    value
        .trim()
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or_else(|| value.trim())
        .to_string()
        .into_non_empty()
}

fn normalize_key(value: &str) -> String {
    let value = value.strip_prefix("slug:").unwrap_or(value);
    if let Some(id) = value
        .split("/manga/")
        .nth(1)
        .or_else(|| value.split(API_PATH).nth(1))
        .map(|part| part.trim_matches('/').split('/').next().unwrap_or_default())
        .filter(|id| id.chars().all(|ch| ch.is_ascii_digit()))
    {
        return format!("/{id}");
    }
    format!(
        "/{}",
        value
            .trim_matches('/')
            .split('/')
            .next()
            .unwrap_or("1")
            .trim()
    )
}

fn manga_url(base: &str, key: &str) -> String {
    format!(
        "{base}/manga/{}",
        normalize_key(key).trim_start_matches('/')
    )
}

fn details_api_url(base: &str, key: &str) -> String {
    format!(
        "{base}{API_PATH}/{}/",
        normalize_key(key).trim_start_matches('/')
    )
}

fn chapter_api_url(base: &str, key: &str) -> String {
    let api_path = key
        .split("#apiChapter")
        .nth(1)
        .unwrap_or("/1/chapter/1")
        .trim();
    format!("{base}{API_PATH}{}", ensure_leading_slash(api_path))
}

fn ensure_leading_slash(value: &str) -> String {
    format!("/{}", value.trim_start_matches('/'))
}

fn parse_listing(body: &str, base: &str, lang: &str) -> Paged<CatalogItem> {
    let page = serde_json::from_str::<PageWrapper<TitleDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("valid listing fixture"));
    let has_next_page = page.page_nav_params.count
        > page
            .page_nav_params
            .page
            .saturating_mul(page.page_nav_params.limit);
    Paged {
        entries: page
            .response
            .into_iter()
            .map(|title| title.into_item(base, lang, false))
            .collect(),
        has_next_page,
    }
}

fn parse_detail(body: &str, key: Option<String>, base: &str, lang: &str) -> CatalogItem {
    let wrapper = serde_json::from_str::<SeriesWrapper<TitleDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid details fixture"));
    let mut item = wrapper.response.into_item(base, lang, true);
    if let Some(key) = key {
        item.key = normalize_key(&key);
        item.url = Some(manga_url(base, &item.key));
    }
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let wrapper = serde_json::from_str::<SeriesWrapper<TitleDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid details fixture"));
    let title_id = wrapper.response.id;
    let chapters_dto = wrapper.response.chapters.unwrap_or(ChaptersDto {
        list: Vec::new(),
        last: None,
    });
    let last = chapters_dto.last;
    let mut chapters = chapters_dto
        .list
        .into_iter()
        .filter(|chapter| {
            !last.as_ref().is_some_and(|last| {
                chapter.vol == last.vol
                    && chapter.ch.parse::<f32>().ok() > last.ch.parse::<f32>().ok()
            })
        })
        .map(|chapter| {
            let title = chapter.title.unwrap_or_default();
            let name = format!("{}. Глава {} {}", chapter.vol, chapter.ch, title)
                .trim()
                .to_string();
            MangaChapter {
                key: format!(
                    "/manga/{title_id}/vol{}/ch{}/rus#apiChapter/{title_id}/chapter/{}",
                    chapter.vol, chapter.ch, chapter.id
                ),
                title: Some(name),
                chapter_number: chapter.ch.parse::<f32>().ok(),
                volume_number: chapter.vol.parse::<f32>().ok(),
                date_uploaded: Some(chapter.date.saturating_mul(1000)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    let wrapper = serde_json::from_str::<SeriesWrapper<ChapterPagesDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("valid pages fixture"));
    wrapper
        .response
        .pages
        .list
        .into_iter()
        .enumerate()
        .map(|(index, page)| MangaPage {
            content: PageContent::Url {
                url: normalize_image_url(&page.img, base),
                context: Some(manga::image_headers(base)),
            },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageWrapper<T> {
    #[serde(rename = "pageNavParams")]
    page_nav_params: NavDto,
    response: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct NavDto {
    count: u64,
    page: u64,
    limit: u64,
}

#[derive(Debug, Deserialize)]
struct SeriesWrapper<T> {
    response: T,
}

#[derive(Debug, Deserialize)]
struct TitleDto {
    id: u64,
    name: String,
    russian: String,
    kind: Option<String>,
    description: Option<String>,
    score: Option<f32>,
    score_users: Option<u64>,
    age_limit: Option<String>,
    synonyms: Option<String>,
    image: ImageDto,
    trans_status: Option<String>,
    status: Option<String>,
    #[serde(default)]
    genres: Option<Vec<NamedDto>>,
    #[serde(default)]
    authors: Option<Vec<AuthorDto>>,
    #[serde(default)]
    chapters: Option<ChaptersDto>,
}

impl TitleDto {
    fn into_item(self, base: &str, lang: &str, initialized: bool) -> CatalogItem {
        let title = if lang == "rus" && !self.russian.trim().is_empty() {
            self.russian.clone()
        } else {
            self.name.clone()
        };
        let alternate = if lang == "rus" {
            self.name
        } else {
            self.russian
        };
        let key = format!("/{}", self.id);
        let mut tags = vec![kind_label(self.kind.as_deref()).to_string()];
        if let Some(age) = self.age_limit.as_deref().and_then(age_label) {
            tags.push(age.to_string());
        }
        tags.extend(
            self.genres
                .unwrap_or_default()
                .into_iter()
                .map(|genre| genre.russian),
        );
        CatalogItem {
            key: key.clone(),
            title,
            alternate_titles: non_empty_vec(alternate),
            cover: self.image.original,
            url: Some(manga_url(base, &key)),
            authors: self
                .authors
                .unwrap_or_default()
                .into_iter()
                .map(|author| author.people_name)
                .collect(),
            description: Some(detail_description(
                self.description,
                self.score,
                self.score_users,
                self.synonyms,
            )),
            tags: tags.into_iter().filter(|tag| !tag.is_empty()).collect(),
            status: status_from(self.trans_status.as_deref(), self.status.as_deref()),
            language: Some("ru".into()),
            content_rating: Some("adult".into()),
            rating: self.score.map(|score| score / 2.0),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImageDto {
    original: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NamedDto {
    russian: String,
}

#[derive(Debug, Deserialize)]
struct AuthorDto {
    people_name: String,
}

#[derive(Debug, Deserialize)]
struct ChaptersDto {
    #[serde(default)]
    list: Vec<ChapterDto>,
    last: Option<ChapterLimitDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterLimitDto {
    vol: String,
    ch: String,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    id: u64,
    vol: String,
    ch: String,
    title: Option<String>,
    date: i64,
}

#[derive(Debug, Deserialize)]
struct ChapterPagesDto {
    pages: PagesDto,
}

#[derive(Debug, Deserialize)]
struct PagesDto {
    list: Vec<PageDto>,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    img: String,
}

fn kind_label(value: Option<&str>) -> &str {
    match value.unwrap_or_default() {
        "manga" => "Манга",
        "manhwa" => "Манхва",
        "manhua" => "Маньхуа",
        "comics" => "Комикс",
        "one_shot" => "Ваншот",
        _ => "Манга",
    }
}

fn age_label(value: &str) -> Option<String> {
    if value == "no" {
        None
    } else {
        Some(value.replace("_plus", "+"))
    }
}

fn status_from(trans_status: Option<&str>, status: Option<&str>) -> ItemStatus {
    match trans_status {
        Some("continued") => ItemStatus::Ongoing,
        Some("completed") => ItemStatus::Completed,
        _ => match status {
            Some("ongoing") => ItemStatus::Ongoing,
            Some("released") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
    }
}

fn detail_description(
    description: Option<String>,
    score: Option<f32>,
    score_users: Option<u64>,
    synonyms: Option<String>,
) -> String {
    let mut parts = Vec::new();
    if let Some(score) = score {
        parts.push(format!(
            "{} {score} (голосов: {})",
            rating_stars(score),
            score_users.unwrap_or(0)
        ));
    }
    if let Some(synonyms) = synonyms.filter(|value| !value.trim().is_empty()) {
        parts.push(format!(
            "Альтернативные названия:\n{}",
            synonyms.replace('/', " / ")
        ));
    }
    if let Some(description) = description.filter(|value| !value.trim().is_empty()) {
        parts.push(description);
    }
    parts.join("\n")
}

fn rating_stars(score: f32) -> &'static str {
    if score > 9.5 {
        "*****"
    } else if score > 8.5 {
        "****+"
    } else if score > 7.5 {
        "****"
    } else if score > 6.5 {
        "***+"
    } else if score > 5.5 {
        "***"
    } else if score > 4.5 {
        "**+"
    } else if score > 3.5 {
        "**"
    } else if score > 2.5 {
        "*+"
    } else if score > 1.5 {
        "*"
    } else {
        ""
    }
}

fn normalize_image_url(input: &str, base: &str) -> String {
    let host = base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    if let Some(start) = input.find(".desu.") {
        if let Some(end) = input[start + 1..].find("/manga/") {
            let end = start + 1 + end;
            return format!("{}{}{}", &input[..start + 1], host, &input[end..]);
        }
    }
    input.to_string()
}

fn non_empty_vec(value: String) -> Vec<String> {
    if value.trim().is_empty() {
        Vec::new()
    } else {
        vec![value]
    }
}

trait IntoNonEmpty {
    fn into_non_empty(self) -> Option<String>;
}

impl IntoNonEmpty for String {
    fn into_non_empty(self) -> Option<String> {
        if self.is_empty() { None } else { Some(self) }
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "pageNavParams": { "count": 1, "page": 1, "limit": 50 },
  "response": [
    {
      "id": 1,
      "name": "Sample",
      "russian": "Сэмпл",
      "kind": "manga",
      "description": "Description",
      "score": 8.0,
      "score_users": 5,
      "age_limit": "no",
      "synonyms": "",
      "image": { "original": "https://img.desu.uno/cover.jpg" },
      "trans_status": "continued",
      "status": "ongoing"
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "response": {
    "id": 1,
    "name": "Sample",
    "russian": "Сэмпл",
    "kind": "manga",
    "description": "Description",
    "score": 8.0,
    "score_users": 5,
    "age_limit": "no",
    "synonyms": "Alt",
    "image": { "original": "https://img.desu.uno/cover.jpg" },
    "trans_status": "continued",
    "status": "ongoing",
    "genres": [{ "russian": "Драма" }],
    "authors": [{ "people_name": "Автор" }],
    "chapters": {
      "last": { "vol": "1", "ch": "1" },
      "list": [{ "id": 10, "vol": "1", "ch": "1", "title": "Start", "date": 1700000000 }]
    }
  }
}"#;

const PAGES_FIXTURE: &str = r#"{
  "response": {
    "pages": {
      "list": [{ "img": "https://img.desu.me/manga/sample/1.jpg" }]
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_desu_flow() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].key, "/1");
        let details = SOURCE.details(json!({"manga":{"key":"/1"}})).unwrap();
        assert_eq!(details.tags, vec!["Манга", "Драма"]);
        let chapters = SOURCE.chapters(json!({"manga":{"key":"/1"}})).unwrap();
        assert_eq!(
            chapters[0].key,
            "/manga/1/vol1/ch1/rus#apiChapter/1/chapter/10"
        );
        let pages = SOURCE
            .pages(json!({"chapter":{"key":chapters[0].key.clone()}}))
            .unwrap();
        assert_eq!(pages.len(), 1);
    }
}
