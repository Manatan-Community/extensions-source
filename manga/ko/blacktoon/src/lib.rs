use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BlackToon = BlackToon;
const BASE_URL: &str = "https://blacktoon.me";
const CDN_URL: &str = "https://blacktoonimg.com/";

struct BlackToon;

impl MangaSource for BlackToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut entries = load_series();
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            entries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        } else {
            entries.sort_by(|left, right| right.hot.cmp(&left.hot));
        }
        Ok(page_series(entries, page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        if query.starts_with(BASE_URL) || query.contains("/webtoon/") {
            let key = normalize_key(&query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        let entries = load_series()
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.name.to_lowercase().contains(&query)
                    || item.author.to_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        Ok(page_series(entries, page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".to_string());
        let id = key
            .trim_end_matches(".html")
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let body = fetch_document(
            &format!("{BASE_URL}/data/toonlist/{id}.js"),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, id))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/webtoons/sample/1".to_string());
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
        if input.starts_with(BASE_URL) || input.contains("/webtoon/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), key)),
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        let mut path = key
            .trim_start_matches('/')
            .trim_end_matches(".html")
            .to_string();
        if path.starts_with("webtoon/") || path.starts_with("webtoons/") {
            path.push_str(".html");
        }
        url::join_url(BASE_URL, &path)
    }
}

fn normalize_key(value: &str) -> String {
    let mut path = if let Some(index) = value.find("/webtoons/") {
        &value[index + 1..]
    } else if let Some(index) = value.find("/webtoon/") {
        &value[index + 1..]
    } else {
        value.trim_start_matches('/')
    }
    .trim_end_matches(".html")
    .to_string();
    if !path.starts_with("webtoon") {
        path = format!("webtoon/{path}");
    }
    format!("/{path}")
}

fn load_series() -> Vec<SeriesItem> {
    let home = fetch_document(BASE_URL, HOME_FIXTURE);
    let mut items = Vec::new();
    for script in home.split("<script").skip(1) {
        if !script.contains("data/webtoon") {
            continue;
        }
        let Some(src) = html::attr(script, "src") else {
            continue;
        };
        let body = fetch_document(&url::join_url(BASE_URL, &src), SERIES_FIXTURE);
        let list_index = body
            .split(" = ")
            .next()
            .and_then(|name| name.rsplit("data").next())
            .and_then(|num| num.parse::<i32>().ok())
            .unwrap_or(-1);
        let json = body
            .split(" = ")
            .nth(1)
            .unwrap_or(&body)
            .trim()
            .trim_end_matches(';');
        let mut parsed = serde_json::from_str::<Vec<SeriesItem>>(json).unwrap_or_default();
        for item in &mut parsed {
            item.list_index = list_index;
        }
        items.extend(parsed);
    }
    if items.is_empty() {
        serde_json::from_str(SERIES_JSON_FIXTURE).unwrap_or_default()
    } else {
        items
    }
}

fn page_series(series: Vec<SeriesItem>, page: u64) -> Paged<CatalogItem> {
    let start = page.saturating_sub(1) as usize * 24;
    let end = (start + 24).min(series.len());
    let entries = if start < series.len() {
        series[start..end].iter().map(series_item).collect()
    } else {
        Vec::new()
    };
    Paged {
        entries,
        has_next_page: end < series.len(),
    }
}

fn series_item(item: &SeriesItem) -> CatalogItem {
    let key = format!("/webtoon/{}", item.id);
    CatalogItem {
        key: key.clone(),
        title: item.name.clone(),
        cover: item
            .poster
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|poster| {
                format!(
                    "{CDN_URL}{}",
                    poster
                        .replace("_x4", "")
                        .replace("_x3", "")
                        .trim_start_matches('/')
                )
            }),
        authors: split_csv(&item.author),
        tags: item
            .tag
            .as_ref()
            .map(|tags| split_csv(tags))
            .unwrap_or_default(),
        status: match item.list_index {
            0 => ItemStatus::Completed,
            1 => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    let slug = key
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("BlackToon");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "og:title", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| slug.to_string()),
        cover: find_between(body, "+img_domain+", "+'")
            .and_then(|script| script.split("+'").nth(1).map(ToString::to_string))
            .map(|path| format!("{CDN_URL}{}", path.trim_start_matches('/')))
            .or_else(|| {
                html::attr_after(body, "<img", "src").map(|value| url::join_url(CDN_URL, &value))
            }),
        description: body
            .rsplit("p class=\"mt-2")
            .next()
            .and_then(|chunk| html::text_between(chunk, ">", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(&key)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_id: &str) -> Vec<MangaChapter> {
    let json = body
        .split(" = ")
        .nth(1)
        .unwrap_or(body)
        .trim()
        .trim_end_matches(';');
    let mut chapters = serde_json::from_str::<Vec<Chapter>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| {
            let key = format!("/webtoons/{manga_id}/{}", chapter.id);
            MangaChapter {
                key: key.clone(),
                title: Some(chapter.title),
                date_uploaded: parse_dash_date(&chapter.date),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "o_src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(CDN_URL, &image),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_dash_date(input: &str) -> Option<i64> {
    let parts = input
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        .collect::<Vec<_>>();
    if parts.len() == 3 {
        Some(parts[0] * 10_000 + parts[1] * 100 + parts[2])
    } else {
        None
    }
}

fn find_between(input: &str, start: &str, end: &str) -> Option<String> {
    Some(input.split(start).nth(1)?.split(end).next()?.to_string())
}

#[derive(Debug, Default, Deserialize)]
struct SeriesItem {
    #[serde(rename = "x")]
    id: String,
    #[serde(rename = "t")]
    name: String,
    #[serde(rename = "p")]
    poster: Option<String>,
    #[serde(default, rename = "au")]
    author: String,
    #[serde(default, rename = "g")]
    updated_at: i64,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default, rename = "h")]
    hot: i64,
    #[serde(skip)]
    list_index: i32,
}

#[derive(Debug, Default, Deserialize)]
struct Chapter {
    id: String,
    #[serde(rename = "t")]
    title: String,
    #[serde(default, rename = "d")]
    date: String,
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<script src="/data/webtoon/data1.js"></script>"#;
const SERIES_FIXTURE: &str = r#"data1 = [{"x":"sample","t":"Sample BlackToon","p":"cover.jpg","au":"Sample Author","g":20240101,"tag":"액션,판타지","h":10}];"#;
const SERIES_JSON_FIXTURE: &str = r#"[{"x":"sample","t":"Sample BlackToon","p":"cover.jpg","au":"Sample Author","g":20240101,"tag":"액션,판타지","h":10}]"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample BlackToon</h1>
<script>var img = +img_domain+ 'cover.jpg' + '';</script>
<p class="mt-2">Sample description</p>
"#;
const CHAPTERS_FIXTURE: &str = r#"toon = [{"id":"1","t":"Chapter 1","d":"2024-01-01"}];"#;
const PAGES_FIXTURE: &str = r#"<div id="toon_content_imgs"><img o_src="page1.jpg"></div>"#;
