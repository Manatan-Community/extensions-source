use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ComX = ComX;
const DEFAULT_BASE_URL: &str = "https://ru.com-x.life";

struct ComX;

impl MangaSource for ComX {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            if page <= 1 { format!("{base}/") } else { format!("{base}/page/{page}/") }
        } else {
            search_url(&base, page, "", request.get("filters").unwrap_or(&Value::Null))
        };
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("http://") || query.starts_with("https://") {
            let key = normalize_key(&base, query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = search_url(&base, page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/reader/1/1".into());
        let body = fetch_document(&base, &absolute_url(&base, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &base, image_domain(&request).as_deref()))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(&base) {
            let key = normalize_key(&base, input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(&base, input, DETAILS_FIXTURE), Some(key), &base)),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base.trim_end_matches('/')))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("domain")).and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn image_domain(request: &Value) -> Option<String> {
    request.get("preferences").and_then(|p| p.get("imageDomain")).and_then(Value::as_str)
        .map(str::trim).filter(|value| !value.is_empty()).map(ToString::to_string)
}

fn search_url(base: &str, page: u64, query: &str, filters: &Value) -> String {
    if !query.is_empty() {
        return format!("{base}/search/{}/page/{page}", url::query_escape(query));
    }
    let page_part = if page > 1 { format!("page/{page}/") } else { String::new() };
    format!(
        "{base}/ComicList/p.cat={}/g={}/t={}/adult={}/{page_part}?dlenewssortby={}&dledirection={}",
        selected_values(filters.get("publishers")).join(","),
        selected_values(filters.get("genres")).join(","),
        selected_values(filters.get("types")).join(","),
        selected_values(filters.get("age")).join(","),
        filter_id(filters, "order").unwrap_or("rating"),
        filter_id(filters, "direction").unwrap_or("desc"),
    )
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).filter_map(option_id).collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value))).filter(|value| !value.is_empty())
}

fn option_id(value: &str) -> Option<String> {
    let id = value.trim().split_once(':').map(|(id, _)| id).unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

fn normalize_key(base: &str, value: &str) -> String {
    let path = value.strip_prefix(base).unwrap_or(value).split('?').next().unwrap_or(value).split('#').next().unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(base: &str, key: &str) -> String {
    url::join_url(base, key)
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let marker = if body.contains("ul#content-load") { "<li" } else { "<div" };
    let entries = body.split(marker).skip(1)
        .filter(|chunk| chunk.contains("short") || chunk.contains("latest") || chunk.contains("readed__title") || chunk.contains("latest__title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "readed__title", "href")
                .or_else(|| html::attr_after(chunk, "latest__title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "readed__title", "</a>")
                .or_else(|| html::text_between(chunk, "latest__title", "</a>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value).replace(" / ", " | ").rsplit(" | ").next().unwrap_or("").trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Комикс".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(base, &image.replace("mini/mini", "mini/mid"))),
                url: Some(absolute_url(base, &key)),
                language: Some("ru".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { has_next_page: body.contains("pagination__btn-loader") || !entries.is_empty(), entries }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    let rating = html::text_between(body, "page__activity-votes", "</").map(|v| html::strip_tags(&v));
    let mut description = String::new();
    if let Some(original) = html::text_between(body, "page__title-original", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()) {
        description.push_str(&original);
        description.push('\n');
    }
    if let Some(rating) = rating.filter(|v| !v.is_empty()) {
        description.push_str(&format!("Рейтинг: {rating}\n"));
    }
    if let Some(text) = html::text_between(body, "page__text", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()) {
        description.push_str(&text);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page__header", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Комикс".into())),
        cover: html::attr_after(body, "img-wide", "data-src").or_else(|| html::attr_after(body, "img-wide", "src")).map(|image| absolute_url(base, &image)),
        authors: info_value(body, "Издатель").into_iter().collect(),
        tags: parse_tags(body),
        status: parse_status(&info_value(body, "Статус").unwrap_or_default()),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        url: Some(absolute_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some(if body.contains("ВНИМАНИЕ! 18+") { "adult" } else { "safe" }.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    let data = body.split("window.__DATA__ = ").nth(1).and_then(|v| v.split("</script>").next()).and_then(|v| v.trim().trim_end_matches(';').parse::<Value>().ok());
    if let Some(data) = data {
        let news_id = data.get("news_id").and_then(Value::as_str).unwrap_or_default();
        if let Some(items) = data.get("chapters").and_then(Value::as_array) {
            return items.iter().filter_map(|item| {
                let id = item.get("id")?.as_str()?;
                let title = item.get("title").and_then(Value::as_str).unwrap_or("Глава");
                Some(MangaChapter {
                    key: format!("/reader/{news_id}/{id}"),
                    title: Some(title.to_string()),
                    chapter_number: item.get("posi").and_then(Value::as_f64).map(|v| v as f32),
                    url: Some(format!("{base}/reader/{news_id}/{id}")),
                    ..MangaChapter::default()
                })
            }).collect();
        }
    }
    Vec::new()
}

fn parse_pages(body: &str, base: &str, forced_domain: Option<&str>) -> Vec<MangaPage> {
    let image_base = forced_domain.map(ToString::to_string)
        .or_else(|| body.split("\"host\":\"").nth(1).and_then(|v| v.split('"').next()).map(|host| format!("https://{host}")))
        .unwrap_or_else(|| base.replacen("https://", "https://img.", 1));
    body.split("\"images\":[").nth(1)
        .and_then(|v| v.split(']').next())
        .unwrap_or("")
        .split(',')
        .map(|v| v.replace(['\\', '"'], "").trim().to_string())
        .filter(|v| !v.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: format!("{}/comix/{}", image_base.trim_end_matches('/'), image.trim_start_matches('/')), context: Some(manga::image_headers(base)) },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<li").find(|chunk| chunk.contains(label)).map(html::strip_tags).map(|v| v.replace(&format!("{label}:"), "").trim().to_string()).filter(|v| !v.is_empty())
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("page__tags").skip(1).flat_map(|chunk| chunk.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>").map(|v| html::strip_tags(&v)))
        .filter(|v| !v.is_empty()).collect()
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("Продолжается") || value.contains("Онгоинг") {
        ItemStatus::Ongoing
    } else if value.contains("Заверш") || value.contains("Лимитка") || value.contains("Ван шот") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"<div class="short"><a href="/comic/sample"><img src="/cover.jpg"></a><div class="readed__title"><a href="/comic/sample">Sample</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="page__grid"><div class="page__header"><h1>Sample</h1></div><div class="img-wide"><img src="/cover.jpg"></div><ul class="page__list"><li>Статус: Продолжается</li></ul><div class="page__text">Description</div></div><script>window.__DATA__ = {"news_id":"1","chapters":[{"id":"1","title":"Глава 1","posi":1}]};</script>"#;
const PAGES_FIXTURE: &str = r#"<script>{"host":"img.ru.com-x.life","images":["1.jpg","2.jpg"]}</script>"#;

export_manga_source!(SOURCE);
