use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ManhwaWeb = ManhwaWeb;
const BASE_URL: &str = "https://manhwaweb.com";
const API_URL: &str = "https://manhwawebbackend-production.up.railway.app";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct ManhwaWeb;

impl MangaSource for ManhwaWeb {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_latest(&fetch_api(
                &format!("{API_URL}/latest/new-manhwa"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_popular(&fetch_api(
                &format!("{API_URL}/manhwa/nuevos"),
                POPULAR_FIXTURE,
            )))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_item(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_api(
            &search_url(page, query, request.get("filters")),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "manhwa/sample".into());
        Ok(details_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "manhwa/sample".into());
        let slug = key.trim_matches('/').rsplit('/').next().unwrap_or(&key);
        let payload: ChapterPayload =
            fetch_json(&format!("{API_URL}/manhwa/see/{slug}"), CHAPTERS_FIXTURE);
        let mut chapters = payload
            .chapters
            .into_iter()
            .filter(|chapter| {
                chapter.created_at.is_some()
                    && (chapter.esp_url.is_some() || chapter.raw_url.is_some())
            })
            .map(|chapter| chapter.to_chapter(&payload.id, &payload.real_id))
            .collect::<Vec<_>>();
        chapters.sort_by(|left, right| {
            right
                .chapter_number
                .partial_cmp(&left.chapter_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manhwa/sample/1".into());
        let slug = key.trim_matches('/').rsplit('/').next().unwrap_or(&key);
        let payload: PagePayload =
            fetch_json(&format!("{API_URL}/chapters/see/{slug}"), PAGES_FIXTURE);
        Ok(payload
            .chapter
            .images
            .into_iter()
            .filter(|image| image.starts_with("http"))
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
            .collect())
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&key)),
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
        .with_cookies_for(API_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    serde_json::from_str(&fetch_api(target, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut out = format!(
        "{API_URL}/manhwa/library?buscar={}&page={}",
        url::query_escape(query),
        page.saturating_sub(1)
    );
    let filters = filters.unwrap_or(&Value::Null);
    for (param, key) in [
        ("tipo", "type"),
        ("demografia", "demography"),
        ("estado", "status"),
        ("erotico", "erotic"),
        ("order_item", "order_item"),
        ("order_dir", "order_dir"),
    ] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            out.push('&');
            out.push_str(param);
            out.push('=');
            out.push_str(&url::query_escape(&value));
        }
    }
    let genres = filter_values(filters, "genres");
    if !genres.is_empty() {
        out.push_str("&generes=");
        out.push_str(&url::query_escape(&genres.join("a")));
    }
    out
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let payload: PopularPayload = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).unwrap());
    let mut comics = payload.top.weekly;
    comics.extend(payload.top.total);
    comics.sort_by(|left, right| right.views.cmp(&left.views));
    Paged {
        entries: comics
            .into_iter()
            .map(PopularComic::to_item)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let payload: LatestPayload = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(LATEST_FIXTURE).unwrap());
    let mut comics = payload.manhwas.esp;
    comics.extend(payload.manhwas.raw18);
    comics.extend(payload.manhwas.esp18);
    comics.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Paged {
        entries: comics
            .into_iter()
            .map(LatestComic::to_item)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let payload: SearchPayload = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
    Paged {
        entries: payload.data.into_iter().map(SearchComic::to_item).collect(),
        has_next_page: payload.next,
    }
}

fn details_item(key: &str) -> CatalogItem {
    let slug = key.trim_matches('/').rsplit('/').next().unwrap_or(key);
    let details: ComicDetails =
        fetch_json(&format!("{API_URL}/manhwa/see/{slug}"), DETAILS_FIXTURE);
    details.to_item(normalize_key(key))
}

fn normalize_key(input: &str) -> String {
    let mut path = input.trim();
    if let Some(rest) = path.strip_prefix(BASE_URL) {
        path = rest;
    }
    path.trim_start_matches('/')
        .trim_end_matches('/')
        .replace("manga/", "manhwa/")
}

fn normalize_timestamp(value: Option<i64>) -> Option<i64> {
    value.map(|timestamp| {
        if timestamp > 99_999_999_999 {
            timestamp / 1000
        } else {
            timestamp
        }
    })
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| {
                value
                    .get("value")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn filter_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| {
                value.as_str().map(ToString::to_string).or_else(|| {
                    value
                        .get("value")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
            })
            .filter(|value| !value.is_empty())
            .collect(),
        Some(value) => filter_string(&serde_json::json!({ key: value }), key)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value])
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct PopularPayload {
    top: PopularData,
}

#[derive(Debug, Deserialize)]
struct PopularData {
    #[serde(rename = "manhwas_esp", default)]
    weekly: Vec<PopularComic>,
    #[serde(rename = "manhwas_raw", default)]
    total: Vec<PopularComic>,
}

#[derive(Debug, Deserialize)]
struct PopularComic {
    #[serde(rename = "link")]
    slug: String,
    #[serde(rename = "numero", default)]
    views: i64,
    name: String,
    #[serde(rename = "imagen")]
    thumbnail: Option<String>,
}

impl PopularComic {
    fn to_item(self) -> CatalogItem {
        item(normalize_key(&self.slug), self.name, self.thumbnail, false)
    }
}

#[derive(Debug, Deserialize)]
struct LatestPayload {
    manhwas: LatestData,
}

#[derive(Debug, Deserialize)]
struct LatestData {
    #[serde(rename = "manhwas_esp", default)]
    esp: Vec<LatestComic>,
    #[serde(rename = "manhwas_raw", default)]
    raw18: Vec<LatestComic>,
    #[serde(rename = "_manhwas", default)]
    esp18: Vec<LatestComic>,
}

#[derive(Debug, Deserialize)]
struct LatestComic {
    #[serde(rename = "create", default)]
    created_at: i64,
    #[serde(rename = "id_rel")]
    slug: String,
    #[serde(rename = "name_manhwa")]
    name: String,
    #[serde(rename = "img")]
    thumbnail: Option<String>,
}

impl LatestComic {
    fn to_item(self) -> CatalogItem {
        item(
            format!("manhwa/{}", self.slug.trim_matches('/')),
            self.name,
            self.thumbnail,
            false,
        )
    }
}

#[derive(Debug, Deserialize)]
struct SearchPayload {
    #[serde(default)]
    data: Vec<SearchComic>,
    #[serde(default)]
    next: bool,
}

#[derive(Debug, Deserialize)]
struct SearchComic {
    #[serde(rename = "real_id")]
    slug: String,
    #[serde(rename = "the_real_name")]
    name: String,
    #[serde(rename = "_imagen")]
    thumbnail: Option<String>,
}

impl SearchComic {
    fn to_item(self) -> CatalogItem {
        item(
            format!("manhwa/{}", self.slug.trim_matches('/')),
            self.name,
            self.thumbnail,
            false,
        )
    }
}

fn item(key: String, title: String, cover: Option<String>, initialized: bool) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized,
        ..CatalogItem::default()
    }
}

#[derive(Debug, Deserialize)]
struct ComicDetails {
    #[serde(rename = "name_esp")]
    title: String,
    #[serde(rename = "_sinopsis")]
    description: Option<String>,
    #[serde(rename = "_status")]
    status: Option<String>,
    #[serde(rename = "_name")]
    alternate_name: Option<String>,
    #[serde(rename = "_imagen")]
    thumbnail: Option<String>,
    #[serde(rename = "_categoris", default)]
    genres: Vec<Value>,
    #[serde(rename = "_extras", default)]
    extras: DetailsExtras,
}

impl ComicDetails {
    fn to_item(self, key: String) -> CatalogItem {
        let mut description = self.description.filter(|value| !value.is_empty());
        let alternate = self.alternate_name.filter(|value| !value.trim().is_empty());
        if let Some(alternate) = alternate.as_ref() {
            description = Some(match description {
                Some(current) => format!("{current}\n\nNombres alternativos: {alternate}"),
                None => format!("Nombres alternativos: {alternate}"),
            });
        }
        CatalogItem {
            key: key.clone(),
            title: self.title,
            alternate_titles: alternate.into_iter().collect(),
            cover: self.thumbnail,
            description,
            authors: self.extras.authors,
            tags: self
                .genres
                .iter()
                .flat_map(|value| value.as_object().into_iter().flat_map(|map| map.values()))
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect(),
            status: match self.status.as_deref() {
                Some("publicandose") => ItemStatus::Ongoing,
                Some("finalizado") => ItemStatus::Completed,
                _ => ItemStatus::Unknown,
            },
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some(LANG.into()),
            content_rating: Some(CONTENT_RATING.into()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct DetailsExtras {
    #[serde(rename = "autores", default)]
    authors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterPayload {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "real_id")]
    real_id: String,
    #[serde(default)]
    chapters: Vec<ChapterDto>,
}

#[derive(Debug, Deserialize)]
struct ChapterDto {
    #[serde(rename = "chapter")]
    number: f32,
    #[serde(rename = "link")]
    esp_url: Option<String>,
    #[serde(rename = "link_raw")]
    raw_url: Option<String>,
    #[serde(rename = "create")]
    created_at: Option<i64>,
}

impl ChapterDto {
    fn to_chapter(self, id: &str, real_id: &str) -> MangaChapter {
        let raw_url = self
            .esp_url
            .as_ref()
            .or(self.raw_url.as_ref())
            .cloned()
            .unwrap_or_default();
        let key = normalize_key(&raw_url.replace(id, real_id));
        MangaChapter {
            key: key.clone(),
            title: Some(format!("Capitulo {}", trim_number(self.number))),
            chapter_number: Some(self.number),
            date_uploaded: normalize_timestamp(self.created_at),
            scanlators: vec![if self.esp_url.is_some() { "Esp" } else { "Raw" }.to_string()],
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct PagePayload {
    #[serde(rename = "chapter")]
    chapter: PageDto,
}

#[derive(Debug, Deserialize)]
struct PageDto {
    #[serde(rename = "img", default)]
    images: Vec<String>,
}

fn trim_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i32)
    } else {
        value.to_string()
    }
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"{"top":{"manhwas_esp":[{"link":"/manhwa/sample","numero":2,"name":"Sample","imagen":"https://img.test/cover.jpg"}],"manhwas_raw":[]}}"#;
const LATEST_FIXTURE: &str = r#"{"manhwas":{"manhwas_esp":[{"create":1713484800,"id_rel":"sample","name_manhwa":"Sample","img":"https://img.test/cover.jpg"}],"manhwas_raw":[],"_manhwas":[]}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":[{"real_id":"sample","the_real_name":"Sample","_imagen":"https://img.test/cover.jpg"}],"next":false}"#;
const DETAILS_FIXTURE: &str = r#"{"name_esp":"Sample","_sinopsis":"Summary","_status":"publicandose","_name":"Alt","_imagen":"https://img.test/cover.jpg","_categoris":[{"1":"Accion"}],"_extras":{"autores":["Author"]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"_id":"old","real_id":"sample","chapters":[{"chapter":1,"link":"/manhwa/old/capitulo-1","create":1713484800}]}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"img":["https://img.test/page1.jpg"]}}"#;
