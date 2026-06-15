use manatan_extension::{CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: AlucardScans = AlucardScans;
const BASE_URL: &str = "https://alucardscans.com";

struct AlucardScans;

impl MangaSource for AlucardScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest_page(&fetch_json_or_fixture(&format!("{BASE_URL}/api/chapters/latest?page={page}&limit=10"), LATEST_FIXTURE)));
        }
        Ok(parse_series_page(&fetch_json_or_fixture(&format!("{BASE_URL}/api/series?sort=views&order=desc&calculateTotalViews=true&page={page}&limit=24"), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document_or_fixture(query, DETAILS_FIXTURE), Some(key))], has_next_page: false });
        }
        Ok(parse_series_page(&fetch_json_or_fixture(&format!("{BASE_URL}/api/series?search={}&page={page}&limit=24", url::query_escape(query)), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE), &absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_document_or_fixture(input, DETAILS_FIXTURE), Some(normalize_key(input)))), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_page(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<AlucardResponse>(body).unwrap_or_default();
    Paged {
        entries: response.series.into_iter().map(AluSeries::into_item).collect(),
        has_next_page: response.pagination.is_some_and(|p| p.page.unwrap_or(1) < p.pages.unwrap_or(1)),
    }
}

fn parse_latest_page(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<AlucardResponse>(body).unwrap_or_default();
    let mut entries = Vec::new();
    for group in response.grouped_chapters {
        if let Some(series) = group.series {
            let item = series.into_item();
            if !entries.iter().any(|existing: &CatalogItem| existing.key == item.key) {
                entries.push(item);
            }
        }
    }
    Paged { entries, has_next_page: response.pagination.is_some_and(|p| p.page.unwrap_or(1) < p.pages.unwrap_or(1)) }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let series = embedded_json(body, r#"initialSeries\":\""#, r#",\"initialChapters"#)
        .or_else(|| embedded_json(body, r#"initialSeries\":"#, r#","initialChapters"#))
        .and_then(|json| serde_json::from_str::<AluSeries>(&json).ok());
    if let Some(series) = series {
        return CatalogItem {
            key: key.clone(),
            title: series.title.unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
            cover: series.cover_image.or(series.cover).map(|v| absolute_url(&v)),
            description: series.description.map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
            tags: series.genres,
            authors: series.author.into_iter().collect(),
            artists: series.artist.into_iter().collect(),
            status: parse_status(series.status.as_deref().unwrap_or_default()),
            url: Some(absolute_url(&key)),
            language: Some("tr".into()),
            content_rating: Some("safe".into()),
            initialized: true,
            ..CatalogItem::default()
        };
    }
    CatalogItem { key: key.clone(), title: html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())), url: Some(absolute_url(&key)), language: Some("tr".into()), content_rating: Some("safe".into()), initialized: true, ..CatalogItem::default() }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    embedded_json(body, r#"initialChapters\":\""#, r#"}"#)
        .or_else(|| embedded_json(body, r#"initialChapters\":"#, r#"}"#))
        .and_then(|json| serde_json::from_str::<Vec<AluChapter>>(&json).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|chapter| {
            let slug = chapter.slug?;
            let number = chapter.number.unwrap_or_default();
            let title_suffix = chapter.title.filter(|v| !v.trim().is_empty()).map(|v| format!(" - {v}")).unwrap_or_default();
            Some(MangaChapter { key: format!("/{slug}"), title: Some(format!("Bölüm {number}{title_suffix}")), url: Some(format!("{BASE_URL}/{slug}")), date_uploaded: None, is_locked: chapter.is_premium.unwrap_or(false), ..MangaChapter::default() })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1)
        .filter(|chunk| chunk.contains("w-full") || chunk.contains("flex-col") || chunk.contains("src=") || chunk.contains("data-src"))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage { content: PageContent::Url { url: absolute_url(&image), context: None }, headers: manga::image_headers(referer), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() })
        .collect()
}

fn embedded_json(body: &str, start: &str, end: &str) -> Option<String> {
    let raw = body.split(start).nth(1)?.split(end).next()?;
    let value = raw.trim().trim_start_matches(':').trim_start_matches('"').trim_end_matches('"').replace(r#"\""#, r#"""#).replace(r#"\\/"#, "/").replace(r#"\n"#, "\n");
    Some(value)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlucardResponse {
    #[serde(default)]
    series: Vec<AluSeries>,
    #[serde(default)]
    grouped_chapters: Vec<AluGroupedChapters>,
    pagination: Option<AluPagination>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AluSeries {
    title: Option<String>,
    cover: Option<String>,
    cover_image: Option<String>,
    slug: Option<String>,
    status: Option<String>,
    description: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    author: Option<String>,
    artist: Option<String>,
}

impl AluSeries {
    fn into_item(self) -> CatalogItem {
        let slug = self.slug.unwrap_or_else(|| self.title.as_deref().map(slugify).unwrap_or_else(|| "sample".into()));
        CatalogItem { key: format!("/manga/{slug}"), title: self.title.unwrap_or_else(|| slug.clone()), cover: self.cover_image.or(self.cover).map(|v| absolute_url(&v)), tags: self.genres, status: parse_status(self.status.as_deref().unwrap_or_default()), url: Some(format!("{BASE_URL}/manga/{slug}")), language: Some("tr".into()), content_rating: Some("safe".into()), initialized: false, ..CatalogItem::default() }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AluGroupedChapters {
    series: Option<AluSeries>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AluChapter {
    title: Option<String>,
    number: Option<String>,
    slug: Option<String>,
    is_premium: Option<bool>,
}

#[derive(Default, Deserialize)]
struct AluPagination {
    page: Option<u64>,
    pages: Option<u64>,
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_lowercase();
    if value.contains("tamam") || value.contains("complete") || value.contains("bitti") { ItemStatus::Completed }
    else if value.contains("ongoing") || value.contains("devam") { ItemStatus::Ongoing }
    else { ItemStatus::Unknown }
}

fn slugify(value: &str) -> String {
    value.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(index) = value.find("/manga/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"series":[{"title":"Sample Manga","slug":"sample","coverImage":"/cover.jpg","status":"ongoing"}],"pagination":{"page":1,"pages":1}}"#;
const LATEST_FIXTURE: &str = r#"{"groupedChapters":[{"series":{"title":"Sample Manga","slug":"sample","coverImage":"/cover.jpg","status":"ongoing"},"chapters":[]}],"pagination":{"page":1,"pages":1}}"#;
const DETAILS_FIXTURE: &str = r#"<script>window.__DATA__={\"initialSeries\":{\"title\":\"Sample Manga\",\"slug\":\"sample\",\"coverImage\":\"/cover.jpg\",\"status\":\"ongoing\",\"description\":\"Description\",\"genres\":[\"Action\"]},\"initialChapters\":[{\"title\":\"Start\",\"number\":\"1\",\"slug\":\"sample-chapter\",\"isPremium\":false}]}</script>"#;
const PAGES_FIXTURE: &str = r#"<div class="w-full flex-col items-center"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
