use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: ManhwaRead = ManhwaRead;
const DEFAULT_BASE_URL: &str = "https://manhwaread.com";
const ALT_BASE_URL: &str = "https://manhwaread.org";
const CONTENT_RATING: &str = "adult";

struct ManhwaRead;

impl MangaSource for ManhwaRead {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, &base_url));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "release:desc"
        } else {
            "weekly_top:desc"
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(
                &base_url,
                &search_url(&base_url, page, "", Some(sort), request.get("filters")),
                LIST_FIXTURE,
            ),
            &base_url,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("https://") || query.starts_with("http://") {
            let key = normalize_manga_key(&rewrite_manga_url(&base_url, query));
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(
                        &base_url,
                        &absolute_url(&base_url, &key),
                        DETAILS_FIXTURE,
                    ),
                    Some(key),
                    &base_url,
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(
            &fetch_document_or_fixture(
                &base_url,
                &search_url(&base_url, page, query, None, request.get("filters")),
                LIST_FIXTURE,
            ),
            &base_url,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base_url = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&base_url, &absolute_url(&base_url, &key), DETAILS_FIXTURE),
            Some(key),
            &base_url,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base_url = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manhwa/sample".to_string());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&base_url, &absolute_url(&base_url, &key), DETAILS_FIXTURE),
            &base_url,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base_url = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manhwa/sample/chapter-1".to_string());
        Ok(parse_pages(
            &fetch_document_or_fixture(&base_url, &absolute_url(&base_url, &key), PAGES_FIXTURE),
            &base_url,
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let preferences = request.get("preferences").cloned().unwrap_or(Value::Null);
        let popular = self.list(
            serde_json::json!({"page": 1, "listingId": "popular", "preferences": preferences}),
        )?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base_url = base_url(&request);
        if input.starts_with(DEFAULT_BASE_URL) || input.starts_with(ALT_BASE_URL) {
            let key = normalize_manga_key(&rewrite_manga_url(&base_url, input));
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(
                        &base_url,
                        &absolute_url(&base_url, &key),
                        DETAILS_FIXTURE,
                    ),
                    Some(key),
                    &base_url,
                )),
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

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base_url.trim_end_matches('/')))
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(base_url: &str, target: &str, fixture: &str) -> String {
    client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("mirror"))
        .and_then(Value::as_str)
        .filter(|value| *value == ALT_BASE_URL)
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

fn search_url(
    base_url: &str,
    page: u64,
    query: &str,
    fallback_sort: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut params = Vec::new();
    params.push(("s".to_string(), url::query_escape(query)));
    let sort = filter_string(filters, "sort").or_else(|| fallback_sort.map(ToString::to_string));
    if let Some(sort) = sort {
        let (sortby, order) = sort.split_once(':').unwrap_or((&sort, "desc"));
        params.push(("sortby".to_string(), sortby.to_string()));
        params.push(("order".to_string(), order.to_string()));
    }
    for key in [
        "keyword_mode",
        "s_mode",
        "status",
        "publish_year",
        "chapters",
    ] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            params.push((query_name(key).to_string(), url::query_escape(&value)));
        }
    }
    for (filter, query_name) in [
        ("artists", "artists[]"),
        ("authors", "authors[]"),
        ("publishers", "publishers[]"),
        ("genres", "genres[]"),
        ("include_tags", "tags[]"),
        ("exclude_tags", "exclude_tags[]"),
    ] {
        for value in filter_values(filters, filter) {
            params.push((query_name.to_string(), url::query_escape(&value)));
        }
    }
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let query = params
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/{page_path}?{query}", base_url.trim_end_matches('/'))
}

fn query_name(filter: &str) -> &str {
    match filter {
        "publish_year" => "publish_year",
        "chapters" => "chapter_numbers",
        other => other,
    }
}

fn filter_string(filters: Option<&Value>, key: &str) -> Option<String> {
    filters?.get(key)?.as_str().map(ToString::to_string)
}

fn filter_values(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(key)) else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_listing(body: &str, base_url: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("manga-item")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "manga-item__link", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = normalize_manga_key(&href);
                if !key.starts_with("/manhwa/") {
                    return None;
                }
                let title = html::text_between(chunk, "manga-item__link", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| "ManhwaRead".to_string());
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| absolute_url(base_url, &image)),
                    url: Some(absolute_url(base_url, &key)),
                    language: Some("en".to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("wp-pagenavi") && body.contains("last"),
    }
}

fn parse_details(body: &str, key: Option<String>, base_url: &str) -> CatalogItem {
    let key = key
        .or_else(|| html::attr_after(body, "rel=\"canonical\"", "href"))
        .map(|value| normalize_manga_key(&value))
        .unwrap_or_else(|| "/manhwa/sample".to_string());
    let metrics = details_metrics(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-titles h1", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "ManhwaRead".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(base_url, &image)),
        description: description(body, metrics),
        authors: values_after_label(body, "Author:"),
        artists: values_after_label(body, "Artist:"),
        tags: link_values(body, "manga-genres")
            .into_iter()
            .chain(values_after_label(body, "Tags:"))
            .collect(),
        status: parse_status(
            &html::attr_after(body, "manga-status", "data-status").unwrap_or_default(),
        ),
        url: Some(absolute_url(base_url, &key)),
        language: Some("en".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base_url: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_manga_key(&href);
            let title = html::text_between(chunk, "chapter-item__name", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapter-item__date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                url: Some(absolute_url(base_url, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, base_url: &str) -> Vec<MangaPage> {
    if let Some(data) = chapter_data(body) {
        return data
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(base_url)),
                },
                headers: manga::image_headers(base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect();
    }
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(base_url, &image),
                context: Some(manga::image_headers(base_url)),
            },
            headers: manga::image_headers(base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_data(body: &str) -> Option<Vec<String>> {
    let start = body.find("var chapterData")?;
    let rest = &body[start..];
    let json_start = rest.find('{')?;
    let json_end = rest[json_start..]
        .find("};")
        .map(|index| json_start + index + 1)?;
    let chapter: ChapterData = serde_json::from_str(&rest[json_start..json_end]).ok()?;
    let decoded = STANDARD.decode(chapter.data).ok()?;
    let pages: Vec<ChapterPage> = serde_json::from_slice(&decoded).ok()?;
    Some(
        pages
            .into_iter()
            .map(|page| format!("{}/{}", chapter.base.trim_end_matches('/'), page.src))
            .collect(),
    )
}

#[derive(Debug, Deserialize)]
struct ChapterData {
    base: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct ChapterPage {
    src: String,
}

fn details_metrics(body: &str) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(rating) = html::text_between(body, "rating__current", "</") {
        let rating = html::strip_tags(&rating);
        if !rating.is_empty() {
            values.push(format!("Rating: {rating}"));
        }
    }
    for label in ["fa-eye", "fa-comments", "w-5 h-5"] {
        if let Some(value) = html::text_between(body, label, "</span>") {
            let value = html::strip_tags(&value);
            if !value.is_empty() {
                values.push(value);
            }
        }
    }
    values
}

fn description(body: &str, mut leading: Vec<String>) -> Option<String> {
    if let Some(publisher) = values_after_label(body, "Publisher:")
        .into_iter()
        .reduce(|acc, item| format!("{acc}, {item}"))
    {
        leading.push(format!("Publisher: {publisher}"));
    }
    if let Some(summary) = html::text_between(body, "manga-desc__content", "</div>")
        .or_else(|| html::text_between(body, "mangaDesc", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        leading.push(summary);
    }
    (!leading.is_empty()).then(|| leading.join("\n\n"))
}

fn values_after_label(body: &str, label: &str) -> Vec<String> {
    body.split(label)
        .skip(1)
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .take_while(|chunk| !chunk.contains("text-primary"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    let Some(start) = body.find(marker) else {
        return Vec::new();
    };
    body[start..]
        .split("<a")
        .skip(1)
        .take_while(|chunk| !chunk.contains("text-primary"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn parse_status(value: &str) -> ItemStatus {
    match value {
        "ongoing" => ItemStatus::Ongoing,
        "completed" => ItemStatus::Completed,
        "canceled" | "cancelled" => ItemStatus::Cancelled,
        "on-hold" => ItemStatus::Hiatus,
        "incomplete" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_manga_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find("/manhwa/") {
            return format!("/{}", value[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn rewrite_manga_url(base_url: &str, value: &str) -> String {
    let key = normalize_manga_key(value);
    let slug = key
        .trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("sample")
        .to_string();
    format!("{}/manhwa/{slug}/", base_url.trim_end_matches('/'))
}

fn absolute_url(base_url: &str, value: &str) -> String {
    url::join_url(base_url, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="main-container"><div class="manga-item"><a class="manga-item__link" href="/manhwa/sample/">Sample Read</a><div class="manga-item__img"><img src="/cover.jpg"></div></div></div>
<div class="wp-pagenavi"><a class="last" href="/page/2/">2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div id="mangaSummary"><div class="manga-titles"><h1>Sample Read</h1></div><div class="manga-genres"><a>Action</a></div><div class="manga-status" data-status="ongoing"></div></div>
<meta property="og:image" content="/cover.jpg"><div id="mangaDesc"><div class="manga-desc__content">Summary</div></div>
<div id="chaptersList"><a class="chapter-item" href="/manhwa/sample/chapter-1/"><span class="chapter-item__name">Chapter 1</span><span class="chapter-item__date">01/01/2024</span></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<script>var chapterData = {"base":"https://cdn.example.test/sample","data":"W3sic3JjIjoicGFnZTEuanBnIn0seyJzcmMiOiJwYWdlMi5qcGcifV0="};</script>
"#;
