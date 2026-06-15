use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: YaoiChan = YaoiChan;
const BASE_URL: &str = "https://yaoi-chan.me";

struct YaoiChan;

impl MangaSource for YaoiChan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "manga/new"
        } else {
            "mostfavorites"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}?offset={}", 20 * page.saturating_sub(1)),
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
        let target = if query.is_empty() {
            catalog_url(page, request.get("filters").unwrap_or(&Value::Null))
        } else {
            format!(
                "{BASE_URL}/?do=search&subaction=search&story={}&search_start={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
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
    let genres = selected_values(filters.get("genres"));
    let excluded = selected_values(filters.get("excludedGenres"));
    let offset = 20 * page.saturating_sub(1);
    let status = filter_id(filters, "status").unwrap_or("");
    let sort = filter_id(filters, "sort").unwrap_or("favdesc");
    let (path, tag_order, status_path) = sort_parts(sort);
    if genres.is_empty() && excluded.is_empty() {
        if status_path && !status.is_empty() {
            format!("{BASE_URL}/{path}/{status}?offset={offset}")
        } else if status.is_empty() {
            format!("{BASE_URL}/{path}?offset={offset}")
        } else {
            format!("{BASE_URL}/{path}?offset={offset}&status={status}")
        }
    } else {
        let mut tags = genres;
        tags.extend(excluded.into_iter().map(|value| format!("-{value}")));
        if status_path && !status.is_empty() {
            return format!(
                "{BASE_URL}/tags/{}/{}/?offset={offset}",
                status,
                tags.join("+")
            );
        }
        let mut params = vec![format!("offset={offset}")];
        if !tag_order.is_empty() {
            params.push(format!("n={tag_order}"));
        }
        if !status.is_empty() {
            params.push(format!("status={status}"));
        }
        format!("{BASE_URL}/tags/{}?{}", tags.join("+"), params.join("&"))
    }
}

fn sort_parts(sort: &str) -> (&'static str, &'static str, bool) {
    match sort {
        "datedesc" => ("manga/new", "", true),
        "dateasc" => ("manga/new&n=dateasc", "dateasc", false),
        "favasc" => ("manga/new&n=favasc", "favasc", false),
        "abcdesc" => ("manga/new&n=abcdesc", "abcdesc", false),
        "chasc" => ("manga/new&n=chasc", "chasc", false),
        "abcasc" => ("catalog", "abcasc", false),
        "chdesc" => ("sortch", "chdesc", false),
        _ => ("mostfavorites", "favdesc", false),
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("content_row")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<h2", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr(chunk, "title")
                    .or_else(|| {
                        html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v))
                    })
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("Вперед") || body.contains("Далее"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    let info = body.split("info_wrap").nth(1).unwrap_or(body);
    let raw_type = info_value(info, "Тип").unwrap_or_default().to_lowercase();
    let mut tags = Vec::new();
    if !raw_type.is_empty() {
        tags.push(raw_type);
    }
    tags.extend(body.split("sidetags").skip(1).flat_map(|chunk| {
        chunk
            .split("<a")
            .skip(1)
            .filter_map(|tag| html::text_between(tag, ">", "</a>").map(|v| html::strip_tags(&v)))
    }));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<title", "</title>")
            .map(|v| {
                html::strip_tags(&v)
                    .split(" »")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .filter(|v| !v.is_empty())
            .or_else(|| html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "YaoiChan".into())),
        cover: html::attr_after(body, "id=\"cover", "src")
            .or_else(|| html::attr_after(body, "id='cover", "src"))
            .map(|image| absolute_url(&image)),
        authors: info_value(info, "Автор").into_iter().collect(),
        tags: tags.into_iter().filter(|v| !v.is_empty()).collect(),
        description: html::text_between(body, "id=\"description", "</div>")
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty()),
        status: parse_status(&info_value(info, "Загружено").unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(2)
        .filter(|chunk| chunk.contains("table_cha"))
        .chain(
            body.split("<tr")
                .skip(1)
                .filter(|chunk| chunk.contains("<a") && chunk.contains("date")),
        )
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Глава".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::text_between(chunk, "date", "</")
                    .map(|v| html::strip_tags(&v))
                    .and_then(|v| dates::parse_ymd(&v)),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let images = html::text_between(body, "fullimg\":[", ",]")
        .map(|v| {
            v.replace('"', "")
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            body.split("<img")
                .skip(1)
                .filter_map(|chunk| {
                    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
                })
                .collect()
        });
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

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(['\n', '<'])
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .filter(|v| !v.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("перевод завершен") {
        ItemStatus::Completed
    } else if lower.contains("перевод продолжается") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    let lower = title.to_lowercase();
    lower
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == "глава" || pair[0] == "часть")
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
        .map(|slug| format!("/{slug}"))
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

const LIST_FIXTURE: &str = r#"<div class="content_row" title="Sample"><h2><a href="/sample">Sample</a></h2><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<title>Sample » YaoiChan</title><img id="cover" src="/cover.jpg"><div id="description">Description</div><table class="table_cha"><tr><td></td></tr><tr><td><a href="/sample/1">Глава 1</a><div class="date">2024-01-01</div></td></tr></table>"#;
const PAGES_FIXTURE: &str = r#"var reader = {"fullimg":["/1.jpg","/2.jpg",]};"#;

export_manga_source!(SOURCE);
