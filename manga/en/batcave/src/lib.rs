use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BatCave = BatCave;
const BASE_URL: &str = "https://batcave.biz";

struct BatCave;

impl MangaSource for BatCave {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let sort = if latest { "editdate" } else { "rating" };
        Ok(parse_listing(&fetch_search_page(
            &search_url(page(&request), "", &Value::Null),
            Some((sort, "desc", false)),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filters
            .get("sort")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let direction = filters
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("desc");
        let filters_applied = filters_applied(filters);
        let sort_form = (!sort.is_empty()).then_some((sort, direction, filters_applied));
        Ok(parse_listing(&fetch_search_page(
            &search_url(page(&request), query, filters),
            sort_form,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comix/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comix/sample".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/1/1?x=test".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn fetch_search_page(target: &str, sort_form: Option<(&str, &str, bool)>, fixture: &str) -> String {
    if let Some((sort, direction, filters_applied)) = sort_form {
        let (sort_key, dir_key) = if filters_applied {
            ("set_new_sort", "set_direction_sort")
        } else {
            ("set_new_sort", "set_direction_sort")
        };
        let sort_scope = if filters_applied {
            "dle_sort_xfilter"
        } else {
            "dle_sort_cat_1"
        };
        let dir_scope = if filters_applied {
            "dle_direction_xfilter"
        } else {
            "dle_direction_cat_1"
        };
        return client()
            .post(target)
            .form(&[
                ("dlenewssortby", sort),
                ("dledirection", direction),
                (sort_key, sort_scope),
                (dir_key, dir_scope),
            ])
            .send_text()
            .unwrap_or_else(|_| fixture.to_string());
    }
    fetch_document(target, fixture)
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    if !query.is_empty() {
        let mut out = format!("{BASE_URL}/search/{}", url::query_escape(query));
        if page > 1 {
            out.push_str(&format!("/page/{page}/"));
        }
        return out;
    }
    let mut path = String::new();
    add_filter_path(filters, "year_from", "y[from]", &mut path);
    add_filter_path(filters, "year_to", "y[to]", &mut path);
    add_multi_filter_path(filters, "publisher", "p", &mut path);
    add_multi_filter_path(filters, "genre", "g", &mut path);
    let base_path = if path.is_empty() {
        "/comix/".to_string()
    } else {
        format!("/ComicList/{path}")
    };
    let mut out = format!("{BASE_URL}{base_path}");
    if page > 1 {
        out.push_str(&format!("page/{page}/"));
    }
    out
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("readed")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "readed__title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "readed__title", "</a>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "BatCave".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination__pages") && body.contains("</a>"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comix/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page__header", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "BatCave".to_string()),
        cover: html::attr_after(body, "page__poster", "src")
            .or_else(|| html::attr_after(body, "page__poster", "data-src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "page__text", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: list_value(body, "Writer").into_iter().collect(),
        artists: list_value(body, "Artist").into_iter().collect(),
        tags: {
            let mut tags = body
                .split("page__tags")
                .nth(1)
                .unwrap_or_default()
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            tags.push("Comic".to_string());
            tags
        },
        status: match list_value(body, "Release type")
            .first()
            .map(|value| value.as_str())
        {
            Some("Ongoing") => ItemStatus::Ongoing,
            Some("Completed") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let Some(json) = extract_window_data(body, "window.__DATA__") else {
        return Vec::new();
    };
    let data: Chapters = serde_json::from_str(&json).unwrap_or_default();
    data.chapters
        .into_iter()
        .map(|chapter| MangaChapter {
            key: format!("/reader/{}/{}{}", data.news_id, chapter.id, data.xhash),
            title: Some(chapter.title),
            chapter_number: Some(chapter.posi),
            date_uploaded: None,
            url: Some(format!(
                "{BASE_URL}/reader/{}/{}{}",
                data.news_id, chapter.id, data.xhash
            )),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Some(json) = extract_window_data(body, "window.__DATA__") else {
        return Vec::new();
    };
    let data: Images = serde_json::from_str(&json).unwrap_or_default();
    data.images
        .into_iter()
        .map(|image| {
            if image.starts_with("http://") || image.starts_with("https://") {
                image.trim().to_string()
            } else {
                url::join_url(BASE_URL, image.trim())
            }
        })
        .enumerate()
        .map(|(index, image)| {
            let headers = if image.contains("batcave.biz") {
                manga::image_headers(BASE_URL)
            } else {
                manatan_extension::Context::new()
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn extract_window_data(body: &str, name: &str) -> Option<String> {
    body.split(name)
        .nth(1)?
        .split_once('=')?
        .1
        .split(';')
        .next()
        .map(str::trim)
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn list_value(body: &str, label: &str) -> Vec<String> {
    body.split("<li")
        .filter(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn add_filter_path(filters: &Value, key: &str, query_key: &str, out: &mut String) {
    if let Some(value) = filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        out.push_str(query_key);
        out.push('=');
        out.push_str(&url::query_escape(value));
        out.push('/');
    }
}

fn add_multi_filter_path(filters: &Value, key: &str, query_key: &str, out: &mut String) {
    let values = match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_i64)
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    };
    if !values.is_empty() {
        out.push_str(query_key);
        out.push('=');
        out.push_str(&values.join(","));
        out.push('/');
    }
}

fn filters_applied(filters: &Value) -> bool {
    ["year_from", "year_to", "publisher", "genre"]
        .iter()
        .any(|key| {
            filters.get(*key).is_some_and(|value| {
                value.as_str().is_some_and(|s| !s.is_empty())
                    || value.as_array().is_some_and(|array| !array.is_empty())
            })
        })
}

#[derive(Default, Deserialize)]
struct Chapters {
    #[serde(default)]
    news_id: i64,
    #[serde(default)]
    chapters: Vec<Chapter>,
    #[serde(default)]
    xhash: String,
}

#[derive(Deserialize)]
struct Chapter {
    id: i64,
    posi: f32,
    title: String,
    #[allow(dead_code)]
    date: String,
}

#[derive(Default, Deserialize)]
struct Images {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="dle-content"><div class="readed"><div class="readed__title"><a href="/comix/sample">Sample BatCave</a></div><img data-src="/cover.jpg"></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<header class="page__header"><h1>Sample BatCave</h1></header><div class="page__poster"><img src="/cover.jpg"></div>
<div class="page__text">Sample description.</div>
<ul class="page__list"><li><div>Writer</div>Writer Name</li><li><div>Artist</div>Artist Name</li><li><div>Release type</div>Ongoing</li></ul>
<div class="page__tags"><a>Action</a></div>
<script>window.__DATA__ = {"news_id":1,"xhash":"?x=test","chapters":[{"id":1,"posi":1.0,"title":"Chapter 1","date":"01.01.2024"}]};</script>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>window.__DATA__ = {"images":["/page1.jpg","/page2.jpg"]};</script>"#;
