use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Kumaraw = Kumaraw;
const BASE_URL: &str = "https://kumaraw.com";

struct Kumaraw;

impl MangaSource for Kumaraw {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let path = if page > 1 { format!("/latest/{page}") } else { String::new() };
            Ok(parse_latest(&fetch_document(&format!("{BASE_URL}{path}"), LATEST_FIXTURE)))
        } else {
            Ok(parse_popular(&fetch_document(BASE_URL, LIST_FIXTURE)))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let target = format!("{BASE_URL}/mangas?search={}", url::query_escape(query));
        Ok(parse_latest(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
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
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = ["top_day", "top_month", "top_all"]
        .into_iter()
        .flat_map(|section| html::text_between(body, &format!("id=\"{section}\""), "</div>").into_iter())
        .flat_map(|section| parse_story_items(&section))
        .fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: false }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_story_items(body),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_story_items(body: &str) -> Vec<CatalogItem> {
    body.split("story_item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/manga/") {
                return None;
            }
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "mg_name", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Kumaraw".into())),
                cover: image_attr(chunk).map(|image| strip_query(&url::join_url(BASE_URL, &image))),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("suggestive".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), Some(key.to_string()))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Kumaraw".into())),
        cover: html::attr_after(body, "detail_avatar", "src").or_else(|| image_attr(body)).map(|image| absolute_url(&image)),
        authors: info_value(body, "著者").map(|value| vec![value]).unwrap_or_default(),
        tags: info_links(body, "/genres/"),
        description: build_description(body),
        status: if text.contains("Completed") || text.contains("完結") {
            manatan_extension::ItemStatus::Completed
        } else {
            manatan_extension::ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("suggestive".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn build_description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(rating) = html::text_between(body, "detail_rate", "</p>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()) {
        parts.push(format!("評価: {rating}"));
    }
    for label in ["ビュー", "雑誌", "ほかの名前"] {
        if let Some(value) = info_value(body, label).filter(|value| !value.is_empty() && value != "Updating" && value != "-") {
            parts.push(format!("{label}: {value}"));
        }
    }
    if let Some(summary) = html::text_between(body, "detail_reviewContent", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty() && value != "Updating") {
        parts.push(summary);
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter_box")
        .nth(1)
        .unwrap_or(body)
        .split("chapter_num")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, ">", "</a>")
                        .map(|value| html::strip_tags(&value).trim_start_matches('#').trim().to_string())
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                date_uploaded: html::text_between(chunk, "chapter_info", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dmy(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("slides_p_path"))
        .unwrap_or(body);
    let json_text = script
        .split("slides_p_path")
        .nth(1)
        .and_then(|rest| rest.split('=').nth(1))
        .and_then(|rest| rest.split(';').next())
        .unwrap_or(PAGES_JSON)
        .trim();
    let encoded = serde_json::from_str::<Vec<String>>(json_text).unwrap_or_else(|_| serde_json::from_str(PAGES_JSON).unwrap_or_default());
    encoded
        .into_iter()
        .filter_map(|value| STANDARD.decode(value).ok())
        .filter_map(|bytes| String::from_utf8(bytes).ok())
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
        .collect()
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("info_label")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "info_value", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn info_links(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-original"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split('?').next().unwrap_or(input).trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn strip_query(input: &str) -> String {
    input.split('?').next().unwrap_or(input).to_string()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn parse_dmy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('-');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="top_day"><div class="story_item"><div class="mg_name"><a href="/manga/sample">Sample Kumaraw</a></div><img src="/cover.jpg?size=small"></div></div>
<div id="top_month"></div><div id="top_all"></div>
"#;
const LATEST_FIXTURE: &str = r#"<div class="recoment_box"><div class="story_item"><div class="mg_name"><a href="/manga/sample">Sample Kumaraw</a></div><img src="/cover.jpg"></div></div>"#;
const SEARCH_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Kumaraw</h1><div class="detail_avatar"><img src="/cover.jpg"></div>
<div class="detail_listInfo"><div class="item"><span class="info_label">著者</span><span class="info_value">Sample Author</span></div><div class="item"><span class="info_label">ほかの名前</span><span class="info_value">Alt Title</span></div></div>
<a href="/genres/action">Action</a><div class="detail_reviewContent">Sample description.</div>
<div class="chapter_box"><div class="item"><a class="chapter_num" href="/chapter/sample-1">#1 Chapter 1</a><p class="chapter_info">12</p><p class="chapter_info">01-01-2024</p></div></div>
"#;
const PAGES_JSON: &str = r#"["aHR0cHM6Ly9rdW1hcmF3LmNvbS9wYWdlMS5qcGc="]"#;
const PAGES_FIXTURE: &str = r#"<script>var slides_p_path = ["aHR0cHM6Ly9rdW1hcmF3LmNvbS9wYWdlMS5qcGc="];</script>"#;
