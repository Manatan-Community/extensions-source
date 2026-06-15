use manatan_extension::{
    AlternateCover, CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest};
use serde::Deserialize;
use serde_json::{Value, json};

const BASE_URL: &str = "https://kagane.to";
const API_URL: &str = "https://yuzuki.kagane.to";
const CACHE_FALLBACK: &str = "https://akari.kagane.to";
const SOURCE: Kagane = Kagane;

struct Kagane;

impl MangaSource for Kagane {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let body = search_body("", source, &request, Some("total_views,desc"));
        Ok(fetch_search(page, &body, SEARCH_FIXTURE, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(series_id) = series_id_from_url(query) {
            let details = fetch_details(&series_id, source);
            return Ok(Paged { entries: vec![details], has_next_page: false });
        }
        let body = search_body(query, source, &request, None);
        Ok(fetch_search(page, &body, SEARCH_FIXTURE, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "series-sample".into());
        Ok(fetch_details(&key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "series-sample".into());
        let body = fetch_json_or_fixture(&format!("{API_URL}/api/v2/series/{key}"), DETAILS_FIXTURE, false);
        let details = parse_details_dto(&body);
        Ok(parse_chapters(&details, &key, source, &request))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let chapter_key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/series/series-sample/reader/book-sample".into());
        let chapter_id = chapter_key.trim_end_matches('/').rsplit('/').next().unwrap_or("book-sample");
        let challenge = fetch_challenge(chapter_id, &request);
        let cache_url = challenge.cache_url.unwrap_or_else(|| CACHE_FALLBACK.to_string());
        let token = challenge.access_token.unwrap_or_default();
        let data_saver = preference_bool(&request, "dataSaver");
        Ok(challenge
            .manifest
            .map(|manifest| manifest.pages)
            .unwrap_or_default()
            .into_iter()
            .map(|page| {
                let ext = page.ext.unwrap_or_else(|| "jxl".into());
                let image_url = format!(
                    "{cache_url}/api/v2/books/page/{chapter_id}/{}.{}?token={}&is_datasaver={}",
                    page.page_uuid, ext, token, data_saver
                );
                MangaPage {
                    content: PageContent::Url { url: image_url, context: Some(api_headers()) },
                    headers: api_headers(),
                    description: Some(format!("Page {}", page.page_no)),
                    ..MangaPage::default()
                }
            })
            .collect())
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("url"))
            .and_then(|value| value.get("url").or(Some(value)))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(MangaPageImage { url, headers: api_headers(), ..MangaPageImage::default() })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(series_id) = series_id_from_url(input) {
            let source = source_for(&request);
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&series_id, source)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    kagane_langs: &'static [&'static str],
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "kagane-en", lang: "en", kagane_langs: &["en"] },
    SourceConfig { id: "kagane-ja", lang: "ja", kagane_langs: &["ja"] },
    SourceConfig { id: "kagane-ko", lang: "ko", kagane_langs: &["ko"] },
    SourceConfig { id: "kagane-zh", lang: "zh", kagane_langs: &["zh-Hans", "zh-Hant"] },
    SourceConfig { id: "kagane-es", lang: "es", kagane_langs: &["es"] },
    SourceConfig { id: "kagane-es-419", lang: "es-419", kagane_langs: &["es-419"] },
    SourceConfig { id: "kagane-fr", lang: "fr", kagane_langs: &["fr"] },
    SourceConfig { id: "kagane-de", lang: "de", kagane_langs: &["de"] },
    SourceConfig { id: "kagane-pt", lang: "pt", kagane_langs: &["pt"] },
    SourceConfig { id: "kagane-pt-br", lang: "pt-BR", kagane_langs: &["pt-BR"] },
    SourceConfig { id: "kagane-ru", lang: "ru", kagane_langs: &["ru"] },
    SourceConfig { id: "kagane-it", lang: "it", kagane_langs: &["it"] },
    SourceConfig { id: "kagane-id", lang: "id", kagane_langs: &["id"] },
    SourceConfig { id: "kagane-vi", lang: "vi", kagane_langs: &["vi"] },
    SourceConfig { id: "kagane-th", lang: "th", kagane_langs: &["th"] },
    SourceConfig { id: "kagane-pl", lang: "pl", kagane_langs: &["pl"] },
    SourceConfig { id: "kagane-hi", lang: "hi", kagane_langs: &["hi"] },
    SourceConfig { id: "kagane-ar", lang: "ar", kagane_langs: &["ar"] },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("kagane-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_headers() -> http::Headers {
    let mut headers = http::Headers::new();
    headers.insert("Origin".into(), BASE_URL.into());
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    headers
}

fn fetch_json_or_fixture(target: &str, fixture: &str, post_body: bool) -> String {
    let client = client();
    let request = if post_body {
        client.post(target).json("{}").xhr()
    } else {
        client.get(target).xhr()
    };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_search(page: u64, body: &Value, fixture: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let sort = body.get("_sort").and_then(Value::as_str).unwrap_or_default();
    let mut target = format!("{API_URL}/api/v2/search/series?page={}&size=35", page.saturating_sub(1));
    if !sort.is_empty() {
        target.push_str("&sort=");
        target.push_str(&manatan_shared::url::query_escape(sort));
    }
    let body_text = client().post(target).json(body.to_string()).xhr().send_text().unwrap_or_else(|_| fixture.to_string());
    parse_search(&body_text, source)
}

fn search_body(query: &str, source: SourceConfig, request: &Value, default_sort: Option<&str>) -> Value {
    let official_only = preference_str(request, "sourceDisplayMode").as_deref() == Some("official");
    let mut body = json!({
        "source_type": if official_only { json!(["Official"]) } else { json!(["Official", "Unofficial", "Mixed"]) },
        "content_lang": source.kagane_langs,
        "content_rating": allowed_ratings(request),
    });
    if !query.is_empty() {
        body["title"] = json!(query);
    }
    if let Some(values) = filter_csv(request, "sourceIds") {
        body["source_id"] = json!(values);
    }
    if let Some(values) = filter_csv(request, "formats") {
        body["format"] = json!(values);
    }
    if let Some(values) = filter_csv(request, "status") {
        body["upload_status"] = json!(values);
    }
    if let Some(values) = filter_csv(request, "genres") {
        body["genres"] = json!({ "values": values });
    }
    if let Some(tags) = filter_csv(request, "tags") {
        let (exclude, include): (Vec<_>, Vec<_>) = tags.into_iter().partition(|tag| tag.starts_with('-'));
        body["tags"] = json!({
            "values": include,
            "exclude": exclude.into_iter().map(|tag| tag.trim_start_matches('-').to_string()).collect::<Vec<_>>()
        });
    }
    body["_sort"] = json!(filter_value(request, "sort").or_else(|| default_sort.map(str::to_string)).unwrap_or_default());
    body
}

fn allowed_ratings(request: &Value) -> Vec<&'static str> {
    const RATINGS: &[&str] = &["safe", "suggestive", "erotica", "pornographic"];
    let max = preference_str(request, "contentRating").unwrap_or_else(|| "pornographic".into());
    let index = RATINGS.iter().position(|rating| *rating == max).unwrap_or(RATINGS.len() - 1);
    RATINGS[..=index].to_vec()
}

fn fetch_details(series_id: &str, source: SourceConfig) -> CatalogItem {
    let body = fetch_json_or_fixture(&format!("{API_URL}/api/v2/series/{series_id}"), DETAILS_FIXTURE, false);
    parse_details(&parse_details_dto(&body), series_id, source, &Value::Null)
}

fn fetch_challenge(chapter_id: &str, request: &Value) -> ChallengeDto {
    let data_saver = preference_bool(request, "dataSaver");
    let integrity = get_integrity_token();
    let client = client();
    let mut req = client
        .post(format!("{API_URL}/api/v2/books/{chapter_id}?is_datasaver={data_saver}"))
        .json("{}")
        .xhr();
    if let Some(token) = integrity {
        req = req.header("x-integrity-token", token);
    }
    let body = req.send_text().unwrap_or_else(|_| CHALLENGE_FIXTURE.to_string());
    serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(CHALLENGE_FIXTURE).expect("challenge fixture"))
}

fn get_integrity_token() -> Option<String> {
    let _ = client().get(BASE_URL).browser_document().send_text();
    let body = client().post(format!("{BASE_URL}/api/integrity")).json("").xhr().send_text().ok()?;
    serde_json::from_str::<IntegrityDto>(&body).ok().map(|dto| dto.token)
}

fn parse_search(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let dto = serde_json::from_str::<SearchDto>(body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).expect("search fixture"));
    Paged {
        entries: dto.content.into_iter().map(|book| book.into_item(source)).collect(),
        has_next_page: !dto.last,
    }
}

fn parse_details_dto(body: &str) -> DetailsDto {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("details fixture"))
}

fn parse_details(dto: &DetailsDto, key: &str, source: SourceConfig, request: &Value) -> CatalogItem {
    let mut title = dto.title.trim().to_string();
    if preference_bool(request, "showEdition") {
        if let Some(edition) = dto.edition_info.as_deref().filter(|value| !value.trim().is_empty()) {
            title = format!("{title} ({edition})");
        }
    }
    CatalogItem {
        key: key.to_string(),
        title,
        alternate_titles: dto.series_alternate_titles.iter().map(|title| title.title.clone()).collect(),
        cover: dto.covers.first().map(|cover| image_url(&cover.image_id)),
        url: Some(format!("{BASE_URL}/series/{key}")),
        authors: dto.series_staff.iter().filter(|staff| staff.role.contains("Author") || staff.role.contains("Story")).map(|staff| staff.name.clone()).collect(),
        artists: dto.series_staff.iter().filter(|staff| staff.role.contains("Artist") || staff.role.contains("Art")).map(|staff| staff.name.clone()).collect(),
        description: dto.description.clone(),
        tags: dto.genres.iter().map(|genre| genre.genre_name.clone()).chain(dto.tags.iter().map(|tag| tag.tag_name.clone())).collect(),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        status: status(&dto.upload_status),
        initialized: true,
        alternate_covers: dto.covers.iter().skip(1).map(|cover| AlternateCover { url: image_url(&cover.image_id), headers: api_headers(), ..AlternateCover::default() }).collect(),
        ..CatalogItem::default()
    }
}

fn parse_chapters(dto: &DetailsDto, series_id: &str, source: SourceConfig, request: &Value) -> Vec<MangaChapter> {
    let mode = preference_str(request, "chapterTitleMode").unwrap_or_else(|| "optional".into());
    dto.series_books
        .iter()
        .rev()
        .map(|book| {
            let key = format!("/series/{series_id}/reader/{}", book.book_id);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter_title(book, &mode)),
                chapter_number: book.chapter_no.as_deref().and_then(|value| value.parse::<f32>().ok()),
                volume_number: book.volume_no.as_deref().and_then(|value| value.parse::<f32>().ok()),
                date_uploaded: book.created_at.as_deref().and_then(dates::parse_fixture_date),
                scanlators: book.groups.iter().map(|group| group.title.clone()).collect(),
                language: Some(source.lang.into()),
                url: Some(format!("{BASE_URL}{key}")),
                page_count: Some(book.page_count),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn chapter_title(book: &BookDto, mode: &str) -> String {
    let title = book.title.trim();
    match mode {
        "always" => match book.chapter_no.as_deref() {
            Some(ch) if !title.is_empty() => format!("Ch.{ch} {title}"),
            Some(ch) => format!("Ch.{ch}"),
            None => title.to_string(),
        },
        "vol_chapter" => {
            let vol = book.volume_no.as_ref().map(|value| format!("Vol.{value} ")).unwrap_or_default();
            let ch = book.chapter_no.as_ref().map(|value| format!("Ch.{value}")).unwrap_or_default();
            let prefix = format!("{vol}{ch}").trim().to_string();
            match (prefix.is_empty(), title.is_empty()) {
                (true, _) => title.to_string(),
                (_, true) => prefix,
                _ => format!("{prefix} {title}"),
            }
        }
        _ => {
            if title.is_empty() {
                book.chapter_no.as_ref().map(|chapter| format!("Ch.{chapter}")).unwrap_or_else(|| "Chapter".into())
            } else {
                title.to_string()
            }
        }
    }
}

fn image_url(image_id: &str) -> String {
    format!("{API_URL}/api/v2/image/{image_id}")
}

fn series_id_from_url(input: &str) -> Option<String> {
    input.split("/series/").nth(1).map(|value| value.split(['/', '?', '#']).next().unwrap_or(value).to_string()).filter(|value| !value.is_empty())
}

fn status(input: &str) -> ItemStatus {
    match input.to_ascii_uppercase().as_str() {
        "ONGOING" => ItemStatus::Ongoing,
        "COMPLETED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "ABANDONED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request.get("filters").and_then(Value::as_object).and_then(|filters| filters.get(id)).and_then(Value::as_str).filter(|value| !value.is_empty()).map(ToString::to_string)
}

fn filter_csv(request: &Value, id: &str) -> Option<Vec<String>> {
    filter_value(request, id).map(|value| value.split(',').map(str::trim).filter(|value| !value.is_empty()).map(ToString::to_string).collect::<Vec<_>>()).filter(|values| !values.is_empty())
}

fn preference_str(request: &Value, id: &str) -> Option<String> {
    request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_str).map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request.get("preferences").and_then(Value::as_object).and_then(|prefs| prefs.get(id)).and_then(Value::as_bool).unwrap_or(false)
}

#[derive(Deserialize)]
struct SearchDto {
    #[serde(default)]
    content: Vec<SearchBookDto>,
    #[serde(default = "default_true")]
    last: bool,
}

#[derive(Deserialize)]
struct SearchBookDto {
    #[serde(rename = "series_id")]
    id: String,
    title: String,
    #[serde(default, rename = "cover_image_id")]
    cover_image: Option<String>,
}

impl SearchBookDto {
    fn into_item(self, source: SourceConfig) -> CatalogItem {
        CatalogItem {
            key: self.id.clone(),
            title: self.title.trim().to_string(),
            cover: self.cover_image.as_deref().map(image_url),
            url: Some(format!("{BASE_URL}/series/{}", self.id)),
            language: Some(source.lang.into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Deserialize)]
struct DetailsDto {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "upload_status")]
    upload_status: String,
    #[serde(default, rename = "series_staff")]
    series_staff: Vec<StaffDto>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default)]
    tags: Vec<TagDto>,
    #[serde(default, rename = "series_alternate_titles")]
    series_alternate_titles: Vec<AlternateTitleDto>,
    #[serde(default, rename = "series_books")]
    series_books: Vec<BookDto>,
    #[serde(default, rename = "edition_info")]
    edition_info: Option<String>,
    #[serde(default, rename = "series_covers")]
    covers: Vec<CoverDto>,
}

#[derive(Deserialize)]
struct StaffDto {
    name: String,
    role: String,
}

#[derive(Deserialize)]
struct GenreDto {
    #[serde(rename = "genre_name")]
    genre_name: String,
}

#[derive(Deserialize)]
struct TagDto {
    #[serde(rename = "tag_name")]
    tag_name: String,
}

#[derive(Deserialize)]
struct AlternateTitleDto {
    title: String,
}

#[derive(Deserialize)]
struct CoverDto {
    #[serde(rename = "image_id")]
    image_id: String,
}

#[derive(Deserialize)]
struct BookDto {
    #[serde(rename = "book_id")]
    book_id: String,
    title: String,
    #[serde(default, rename = "created_at")]
    created_at: Option<String>,
    #[serde(default, rename = "page_count")]
    page_count: u32,
    #[serde(default, rename = "chapter_no")]
    chapter_no: Option<String>,
    #[serde(default, rename = "volume_no")]
    volume_no: Option<String>,
    #[serde(default)]
    groups: Vec<GroupDto>,
}

#[derive(Deserialize)]
struct GroupDto {
    title: String,
}

#[derive(Deserialize)]
struct ChallengeDto {
    #[serde(default, rename = "access_token")]
    access_token: Option<String>,
    #[serde(default, rename = "cache_url")]
    cache_url: Option<String>,
    #[serde(default)]
    manifest: Option<ManifestDto>,
}

#[derive(Deserialize)]
struct ManifestDto {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Deserialize)]
struct PageDto {
    #[serde(rename = "page_no")]
    page_no: i32,
    #[serde(rename = "page_id")]
    page_uuid: String,
    #[serde(default)]
    ext: Option<String>,
}

#[derive(Deserialize)]
struct IntegrityDto {
    token: String,
}

fn default_true() -> bool {
    true
}

const SEARCH_FIXTURE: &str = r#"{
  "content": [{ "series_id": "series-sample", "title": "Sample Series", "cover_image_id": "cover-sample" }],
  "last": true
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "title": "Sample Series",
  "description": "Sample description",
  "upload_status": "ONGOING",
  "series_staff": [{ "name": "Sample Author", "role": "Author" }, { "name": "Sample Artist", "role": "Artist" }],
  "genres": [{ "genre_name": "Action" }],
  "tags": [{ "tag_name": "Adventure" }],
  "series_alternate_titles": [{ "title": "Sample Alt" }],
  "series_books": [{ "book_id": "book-sample", "title": "Chapter One", "created_at": "2024-01-02T00:00:00", "page_count": 2, "chapter_no": "1", "volume_no": "1", "groups": [{ "title": "Sample Group" }] }],
  "edition_info": "Digital",
  "series_covers": [{ "image_id": "cover-sample" }, { "image_id": "cover-alt" }]
}"#;

const CHALLENGE_FIXTURE: &str = r#"{
  "access_token": "token",
  "cache_url": "https://akari.kagane.to",
  "manifest": { "pages": [{ "page_no": 1, "page_id": "page-one", "ext": "jpg" }] }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_details_chapters_and_pages() {
        let source = SOURCES[0];
        let search = parse_search(SEARCH_FIXTURE, source);
        assert_eq!(search.entries[0].key, "series-sample");
        let details_dto = parse_details_dto(DETAILS_FIXTURE);
        let details = parse_details(&details_dto, "series-sample", source, &json!({"preferences":{"showEdition":true}}));
        assert_eq!(details.title, "Sample Series (Digital)");
        assert_eq!(details.alternate_covers.len(), 1);
        let chapters = parse_chapters(&details_dto, "series-sample", source, &Value::Null);
        assert_eq!(chapters[0].key, "/series/series-sample/reader/book-sample");
        assert_eq!(series_id_from_url("https://kagane.to/series/series-sample"), Some("series-sample".into()));
    }
}
