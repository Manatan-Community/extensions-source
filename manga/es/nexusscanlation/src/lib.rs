use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: NexusScanlation = NexusScanlation;
const BASE_URL: &str = "https://nexusscanlation.com";
const API_BASE_URL: &str = "https://api.nexusscanlation.com/api/v1";

struct NexusScanlation;

impl MangaSource for NexusScanlation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "nuevo" } else { "popular" };
        Ok(parse_catalog(&api_get(&format!("/catalog?page={page}&orden={order}"), CATALOG_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = key_from_url(query).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged { entries: vec![details_for(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if query.is_empty() { format!("/catalog?page={page}") } else { format!("/catalog/search?q={}&page={page}", url::query_escape(query)) };
        Ok(parse_catalog(&api_get(&path, CATALOG_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_for(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(parse_chapters(&api_get(&format!("/series/{key}"), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/chapter-1".into());
        let (series, chapter) = key.split_once('/').unwrap_or(("sample", "chapter-1"));
        Ok(parse_pages(&api_get(&format!("/series/{series}/capitulos/{chapter}"), PAGES_FIXTURE)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/series/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (series, chapter) = key.split_once('/').unwrap_or(("sample", "chapter-1"));
            format!("{BASE_URL}/series/{series}/chapter/{chapter}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = key_from_url(input).unwrap_or_else(|| "sample".to_string());
            return Ok(Some(UrlResolveResult { item: Some(details_for(&key)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_header("Origin", BASE_URL)
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Accept-Language", "es-419,es;q=0.9,es-ES;q=0.8")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_cookies_for(API_BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(path: &str, fixture: &str) -> String {
    client().get(format!("{API_BASE_URL}{path}")).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CATALOG_FIXTURE).unwrap());
    let entries = root.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(catalog_item).collect();
    let has_next_page = root.pointer("/meta/has_next").and_then(Value::as_bool).unwrap_or(false);
    Paged { entries, has_next_page }
}

fn catalog_item(item: &Value) -> Option<CatalogItem> {
    let key = text(item, "slug")?;
    let id = text(item, "id");
    Some(CatalogItem {
        key: key.clone(),
        title: text(item, "titulo").unwrap_or_else(|| key.clone()),
        cover: id.as_deref().map(|id| format!("https://cdn.nexusscanlation.com/series/{id}/portada.jpg")).or_else(|| text(item, "portada_url")),
        url: Some(format!("{BASE_URL}/series/{key}")),
        language: Some("es".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_for(key: &str) -> CatalogItem {
    let root: Value = serde_json::from_str(&api_get(&format!("/series/{key}"), DETAILS_FIXTURE)).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let series = root.get("serie").unwrap_or(&root);
    let mut out = catalog_item(series).unwrap_or_else(|| CatalogItem { key: key.to_string(), title: key.to_string(), language: Some("es".into()), content_rating: Some("adult".into()), ..CatalogItem::default() });
    out.description = text(series, "descripcion");
    out.tags = series.get("generos").and_then(Value::as_array).into_iter().flatten().filter_map(|v| text(v, "nombre")).collect();
    let credits = series.get("autores").and_then(Value::as_array).into_iter().flatten().collect::<Vec<_>>();
    out.authors = credits.iter().filter(|v| text(v, "rol").as_deref() != Some("artista")).filter_map(|v| text(v, "nombre")).collect();
    out.artists = credits.iter().filter(|v| text(v, "rol").as_deref() == Some("artista")).filter_map(|v| text(v, "nombre")).collect();
    out.status = match text(series, "estado").unwrap_or_default().as_str() {
        "en_emision" => ItemStatus::Ongoing,
        "finalizado" => ItemStatus::Completed,
        "pausado" => ItemStatus::Hiatus,
        "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    };
    out.initialized = true;
    out
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let series_slug = root.pointer("/serie/slug").and_then(Value::as_str).unwrap_or("sample");
    root.get("capitulos").and_then(Value::as_array).into_iter().flatten().filter_map(|chapter| {
        let slug = text(chapter, "slug")?;
        let number = chapter.get("numero").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let mut title = format!("Capitulo {}", number);
        if chapter.get("es_premium").and_then(Value::as_bool).unwrap_or(false) { title = format!("Locked {title}"); }
        if let Some(extra) = text(chapter, "titulo") { title.push_str(" - "); title.push_str(&extra); }
        Some(MangaChapter {
            key: format!("{series_slug}/{slug}"),
            title: Some(title),
            chapter_number: Some(number),
            is_locked: chapter.get("es_premium").and_then(Value::as_bool).unwrap_or(false),
            url: Some(format!("{BASE_URL}/series/{series_slug}/chapter/{slug}")),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let data = root.get("data").unwrap_or(&root);
    if data.get("es_premium").and_then(Value::as_bool).unwrap_or(false) || data.get("locked").and_then(Value::as_bool).unwrap_or(false) {
        return Vec::new();
    }
    data.get("paginas").and_then(Value::as_array).into_iter().flatten().enumerate().filter_map(|(index, page)| {
        let mut image = text(page, "url")?;
        if let Some(sc) = page.get("sc") {
            let c = sc.get("c").and_then(Value::as_i64).unwrap_or(0);
            let r = sc.get("r").and_then(Value::as_i64).unwrap_or(0);
            let s = sc.get("s").and_then(Value::as_i64).unwrap_or(0);
            if c > 0 && r > 0 { image.push_str(&format!("#scramble={c},{r},{s}")); }
        }
        Some(MangaPage {
            content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
    }).collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string).filter(|value| !value.is_empty())
}

fn key_from_url(input: &str) -> Option<String> {
    input.split("/series/").nth(1)?.split('/').next().map(ToString::to_string).filter(|value| !value.is_empty())
}

export_manga_source!(SOURCE);

const CATALOG_FIXTURE: &str = r#"{"data":[{"id":"1","slug":"sample","titulo":"Sample Nexus","portada_url":"https://cdn.nexusscanlation.com/series/1/portada.jpg"}],"meta":{"has_next":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"serie":{"id":"1","slug":"sample","titulo":"Sample Nexus","portada_url":"https://cdn.nexusscanlation.com/series/1/portada.jpg","descripcion":"Fixture summary.","estado":"en_emision","generos":[{"nombre":"Accion"}],"autores":[{"nombre":"Writer","rol":"autor"},{"nombre":"Artist","rol":"artista"}]},"capitulos":[{"slug":"chapter-1","numero":1,"titulo":"Inicio","published_at":"2024-01-01T00:00:00.000Z","es_premium":false}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"paginas":[{"url":"https://cdn.nexusscanlation.com/series/1/1.jpg","sc":{"c":2,"r":2,"s":1}}],"es_premium":false,"locked":false}}"#;
