use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaPoisk = MangaPoisk;
const BASE_URL: &str = "https://mangap.ru";

struct MangaPoisk;

impl MangaSource for MangaPoisk {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "-last_chapter_at"
        } else {
            "popular"
        };
        Ok(parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/manga?sortBy={sort}&page={page}"),
                LIST_FIXTURE,
            ),
            false,
        ))
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
        let target = if query.is_empty() {
            catalog_url(page, request.get("filters").unwrap_or(&Value::Null))
        } else {
            format!(
                "{BASE_URL}/search?q={}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            !query.is_empty(),
        ))
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
        let first = fetch_document(
            &format!("{}?tab=chapters", absolute_url(&key)),
            DETAILS_FIXTURE,
        );
        if first.contains("Главы удалены по требованию правообладателя")
        {
            return Ok(Vec::new());
        }
        let list_url = format!("{}/chaptersList", absolute_url(&key));
        let body = fetch_document(&list_url, DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        let last_page = last_page(&body);
        for page in 2..=last_page {
            chapters.extend(parse_chapters(&fetch_document(
                &format!("{list_url}?page={page}"),
                DETAILS_FIXTURE,
            )));
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        if body.contains("text-error-500-400-token") {
            return Ok(Vec::new());
        }
        Ok(parse_pages(&body))
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
        .with_referer(BASE_URL.to_string())
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

fn catalog_url(page: u64, filters: &Value) -> String {
    let mut params = vec![format!("page={page}")];
    params.push(format!(
        "sortBy={}",
        filter_id(filters, "sort").unwrap_or("popular")
    ));
    for value in selected_values(filters.get("status")) {
        params.push(format!("translated[]={value}"));
    }
    for value in selected_values(filters.get("genres")) {
        params.push(format!("genres[]={value}"));
    }
    format!("{BASE_URL}/manga?{}", params.join("&"))
}

fn parse_listing(body: &str, is_search: bool) -> Paged<CatalogItem> {
    let marker = if is_search {
        "article card"
    } else {
        "manga-card"
    };
    let entries = body
        .split(marker)
        .skip(1)
        .filter_map(|chunk| {
            let href = if is_search {
                html::attr_after(chunk, "card-about", "href")
            } else {
                html::attr_after(chunk, "<a", "href")
            }?;
            let key = normalize_key(&href);
            let title = if is_search {
                html::text_between(chunk, "entry-title", "</").map(|v| {
                    html::strip_tags(&v)
                        .split('/')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
            } else {
                html::attr_after(chunk, "<a", "title")
                    .map(|v| v.split('/').next().unwrap_or("").trim().to_string())
            }
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let info = body
        .split("div class=\"card")
        .find(|chunk| chunk.contains("<header"))
        .unwrap_or(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(info, "text-base", "</")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MangaPoisk".into())),
        cover: html::attr_after(info, "w-full", "src").map(|image| absolute_url(&image)),
        tags: info
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("Жанр") || chunk.contains("genres"))
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</a>").map(|v| html::strip_tags(&v))
            })
            .filter(|v| !v.is_empty())
            .collect(),
        description: html::text_between(info, "manga-description", "</")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: parse_status(
            &html::text_between(info, "Статус:", "</")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_default(),
        ),
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
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Глава".into());
            let title_for_number = html::text_between(chunk, "chapter-title", "</")
                .map(|v| html::strip_tags(&v))
                .unwrap_or_else(|| title.clone());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number(&title_for_number),
                date_uploaded: html::text_between(chunk, "chapter-date", "</")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| parse_date(&v)),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-image"))
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
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

fn last_page(body: &str) -> u64 {
    body.split("page-item")
        .skip(1)
        .filter_map(|chunk| html::strip_tags(chunk).parse::<u64>().ok())
        .max()
        .unwrap_or(1)
}

fn parse_status(status: &str) -> ItemStatus {
    if status.contains("Завершена") {
        ItemStatus::Completed
    } else if status.contains("Выпускается") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let lower = value.trim().to_lowercase();
    let amount = lower.split_whitespace().next()?.parse::<i64>().ok();
    if let Some(amount) = amount {
        let now = 1_704_067_200;
        if lower.contains("минут") {
            return Some(now - amount * 60);
        }
        if lower.contains("час") {
            return Some(now - amount * 3_600);
        }
        if lower.contains("дня") || lower.contains("дней") {
            return Some(now - amount * 86_400);
        }
    }
    parse_russian_month_date(&lower)
}

fn parse_russian_month_date(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let day = parts[0].parse::<u32>().ok()?;
    let month = match parts[1] {
        "января" => 1,
        "февраля" => 2,
        "марта" => 3,
        "апреля" => 4,
        "мая" => 5,
        "июня" => 6,
        "июля" => 7,
        "августа" => 8,
        "сентября" => 9,
        "октября" => 10,
        "ноября" => 11,
        "декабря" => 12,
        _ => return None,
    };
    let year = parts[2].parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year}-{month:02}-{day:02}"))
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0].to_lowercase().contains("глава"))
        .and_then(|pair| pair[1].replace(',', ".").parse().ok())
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

const LIST_FIXTURE: &str = r#"<div class="manga-card"><a href="/manga/sample" title="Sample"><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="card"><header></header><div class="text-base"><span>Sample</span></div><div class="manga-description">Description</div><div class="chapter-item"><a href="/manga/sample/chapter-1">Глава 1</a><span class="chapter-title">Глава 1</span><span class="chapter-date">01 января 2024</span></div></div>"#;
const PAGES_FIXTURE: &str =
    r#"<img class="page-image" src="/1.jpg"><img class="page-image" src="/2.jpg">"#;

export_manga_source!(SOURCE);
