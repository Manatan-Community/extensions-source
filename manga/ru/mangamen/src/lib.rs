use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaMen = MangaMen;
const BASE_URL: &str = "https://mangamen.com";

struct MangaMen;

impl MangaSource for MangaMen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "last_chapter_at"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga-list?sort={sort}&dir=desc&page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("slug:") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &catalog_url(page, query, request.get("filters").unwrap_or(&Value::Null)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_url(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![format!("page={page}")];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    params.push(format!(
        "sort={}",
        filter_id(filters, "sort").unwrap_or("last_chapter_at")
    ));
    params.push(format!(
        "dir={}",
        filter_id(filters, "dir").unwrap_or("desc")
    ));
    for value in selected_values(filters.get("genres")) {
        params.push(format!("genres[include][]={}", url::query_escape(&value)));
    }
    for value in selected_values(filters.get("excludedGenres")) {
        params.push(format!("genres[exclude][]={}", url::query_escape(&value)));
    }
    format!("{BASE_URL}/manga-list?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("media-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "media-card__title", "</")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr(chunk, "data-src")
                    .or_else(|| background_url(chunk))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: entries.len() >= 30 || body.contains("page="),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let rows = info_rows(body);
    let title = html::text_between(body, "itemprop=name", "</")
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|v| html::strip_tags(&v))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaMen".into()));
    let synopsis = html::text_between(body, "info-desc__content", "</")
        .map(|v| html::strip_tags(&v))
        .unwrap_or_default();
    let alt = html::text_between(body, "alternativeHeadline", "</")
        .map(|v| html::strip_tags(&v))
        .unwrap_or_default();
    let mut description = synopsis;
    if !alt.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&format!("Альтернативные названия: {alt}"));
    }
    for label in [
        "Издатель",
        "Статус перевода",
        "Дата релиза",
        "Формат выпуска",
        "Загружено глав",
        "Просмотров",
        "Рейтинг",
    ] {
        if let Some(value) = rows
            .iter()
            .find(|(k, _)| k == label)
            .map(|(_, v)| v)
            .filter(|v| !v.is_empty())
        {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(&format!("{label}: {value}"));
        }
    }
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "manga__cover", "data-src")
            .or_else(|| html::attr_after(body, "manga__image", "data-src"))
            .or_else(|| html::attr_after(body, "property=\"og:image", "content"))
            .map(|image| absolute_url(&image)),
        authors: row_value(&rows, "Автор").into_iter().collect(),
        artists: row_value(&rows, "Художник")
            .or_else(|| row_value(&rows, "Автор"))
            .into_iter()
            .collect(),
        tags: parse_tags(body, &rows),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        status: parse_status(&row_value(&rows, "Статус тайтла").unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-item__name", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-item__name", "</")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    let volume = html::attr(chunk, "data-volume").unwrap_or_else(|| "1".into());
                    let number = html::attr(chunk, "data-number").unwrap_or_else(|| "1".into());
                    format!("Том {volume}. Глава {number}")
                });
            let number = html::attr(chunk, "data-number")
                .and_then(|v| v.parse().ok())
                .or_else(|| chapter_number(&title));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: number,
                date_uploaded: html::text_between(chunk, "chapter-item__date", "</")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| parse_date(&v)),
                scanlators: html::text_between(chunk, "chapter-item__added", "</")
                    .map(|v| html::strip_tags(&v))
                    .filter(|v| !v.is_empty())
                    .into_iter()
                    .collect(),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let array =
        html::text_between(body, "window.__pg", "</script>").unwrap_or_else(|| body.to_string());
    let mut images = Vec::new();
    let mut rest = array.as_str();
    while let Some(start) = rest.find("\"u\":\"") {
        rest = &rest[start + 5..];
        let Some(end) = rest.find('"') else {
            break;
        };
        images.push(rest[..end].replace("\\/", "/"));
        rest = &rest[end..];
    }
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
            .collect();
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_rows(body: &str) -> Vec<(String, String)> {
    body.split("info-list__row")
        .skip(1)
        .filter_map(|chunk| {
            let key =
                html::text_between(chunk, "<strong", "</strong>").map(|v| html::strip_tags(&v))?;
            let value = html::text_between(chunk, "<span", "</span>")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_default();
            Some((key, value))
        })
        .collect()
}

fn row_value(rows: &[(String, String)], key: &str) -> Option<String> {
    rows.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
}

fn parse_tags(body: &str, rows: &[(String, String)]) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(kind) = row_value(rows, "Тип") {
        tags.push(kind);
    }
    tags.extend(
        body.split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genres[include]") || chunk.contains("tags[include]"))
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</a>").map(|v| html::strip_tags(&v))
            }),
    );
    tags.sort();
    tags.dedup();
    tags
}

fn parse_status(raw: &str) -> ItemStatus {
    match raw.to_lowercase().as_str() {
        "онгоинг" | "продолжается" => ItemStatus::Ongoing,
        "завершён" | "завершен" | "закончен" => ItemStatus::Completed,
        "приостановлен" | "заморожен" => ItemStatus::Hiatus,
        "заброшен" | "выпуск прекращён" | "выпуск прекращен" => {
            ItemStatus::Cancelled
        }
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.len() == 10 && trimmed.chars().nth(2) == Some('.') {
        let mut parts = trimmed.split('.');
        return dates::parse_ymd(&format!(
            "{}-{}-{}",
            parts.nth(2)?,
            parts.next()?,
            parts.next()?
        ));
    }
    None
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0].to_lowercase().contains("глава"))
        .and_then(|pair| pair[1].replace(',', ".").parse().ok())
}

fn background_url(value: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let end = value[start..].find(')')? + start;
    Some(value[start..end].trim_matches(['"', '\'']).to_string())
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

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value)))
        .filter(|value| !value.is_empty())
}

fn option_id(value: &str) -> Option<String> {
    let id = value
        .trim()
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

fn normalize_key(value: &str) -> String {
    let value = value
        .strip_prefix("slug:")
        .map(|slug| format!("/manga/{slug}"))
        .unwrap_or_else(|| value.to_string());
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(&value)
        .split('?')
        .next()
        .unwrap_or(&value)
        .split('#')
        .next()
        .unwrap_or(&value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

const LIST_FIXTURE: &str = r#"<a class="media-card" href="/manga/sample" data-src="/cover.jpg"><span class="media-card__title">Sample</span></a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 itemprop="name">Sample</h1><div class="info-desc__content">Description</div><div class="chapter-item" data-number="1"><div class="chapter-item__name"><a href="/manga/sample/chapter-1">Глава 1</a></div><div class="chapter-item__date">01.01.2024</div></div>"#;
const PAGES_FIXTURE: &str =
    r#"<script>window.__pg = [{"p":1,"u":"/1.jpg"},{"p":2,"u":"/2.jpg"}]</script>"#;

export_manga_source!(SOURCE);
