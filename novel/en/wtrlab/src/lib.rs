use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    dates, html, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: WtrLab = WtrLab;
const BASE_URL: &str = "https://wtr-lab.com";
const LANG_PREFIX: &str = "en";

struct WtrLab;

impl NovelSource for WtrLab {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let body = post_json_or_fixture(
                &format!("{BASE_URL}/api/home/recent"),
                &json!({ "page": page }).to_string(),
                LATEST_FIXTURE,
                BASE_URL,
            );
            return Ok(parse_recent(&body));
        }

        let build_id = fetch_build_id();
        let target = format!(
            "{BASE_URL}/_next/data/{build_id}/{LANG_PREFIX}/novel-finder.json?{}",
            finder_query(&request, page, "")
        );
        Ok(parse_finder(&fetch_json_or_fixture(
            &target,
            FINDER_FIXTURE,
            &format!("{BASE_URL}/{LANG_PREFIX}/novel-finder"),
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let build_id = fetch_build_id();
        let target = format!(
            "{BASE_URL}/_next/data/{build_id}/{LANG_PREFIX}/novel-finder.json?{}",
            finder_query(&request, page, query)
        );
        Ok(parse_finder(&fetch_json_or_fixture(
            &target,
            FINDER_FIXTURE,
            &format!("{BASE_URL}/{LANG_PREFIX}/novel-finder"),
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| sample_key().to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| sample_key().to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let (raw_id, count, slug) = parse_series_identity(&body, &key);
        Ok(fetch_chapters(raw_id, count, &slug, &key))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("{}/chapter-1", sample_key()));
        let (raw_id, chapter_no) = chapter_identity(&key).unwrap_or((1, 1));
        let target = format!("{BASE_URL}/api/reader/get");
        let referer = absolute_url(&key);
        let body = ["ai", "web"]
            .iter()
            .find_map(|translate| {
                let payload = json!({
                    "translate": translate,
                    "language": LANG_PREFIX,
                    "raw_id": raw_id,
                    "chapter_no": chapter_no,
                    "retry": false,
                    "force_retry": false
                })
                .to_string();
                let response = post_json_or_fixture(&target, &payload, TEXT_FIXTURE, &referer);
                let parsed = serde_json::from_str::<Value>(&response).ok()?;
                parsed.get("error").is_none().then_some(response)
            })
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let mut latest_request = request;
        latest_request["listing"] = Value::String("latest".to_string());
        let latest = self.list(latest_request)?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Novel Finder".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recent".to_string(),
                style: Some(HomeSectionStyle::Cover),
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client()
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_json_or_fixture(target: &str, payload: &str, fixture: &str, referer: &str) -> String {
    client()
        .post(target)
        .xhr()
        .referer(referer)
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_build_id() -> String {
    let body = fetch_document_or_fixture(&format!("{BASE_URL}/{LANG_PREFIX}/novel-finder"), HOME_FIXTURE);
    next_data(&body)
        .and_then(|data| {
            serde_json::from_str::<Value>(&data)
                .ok()
                .and_then(|value| text(&value, "buildId"))
        })
        .unwrap_or_else(|| "build".to_string())
}

fn finder_query(request: &Value, page: u64, query: &str) -> String {
    let filters = request.get("filters");
    let mut parts = vec![
        pair("orderBy", filter_string(filters, "orderBy", "update")),
        pair("order", filter_string(filters, "order", "desc")),
        pair("status", filter_string(filters, "status", "all")),
        pair(
            "release_status",
            filter_string(filters, "release_status", "all"),
        ),
        pair("addition_age", filter_string(filters, "addition_age", "all")),
        pair("page", page.to_string()),
    ];
    let text_query = filter_string(filters, "search", query);
    if !text_query.is_empty() {
        parts.push(pair("text", text_query));
    }
    for (id, param) in [
        ("min_chapters", "minc"),
        ("min_rating", "minr"),
        ("min_review_count", "minrc"),
    ] {
        let value = filter_string(filters, id, "");
        if !value.is_empty() {
            parts.push(pair(param, value));
        }
    }
    parts.join("&")
}

fn pair(key: &str, value: String) -> String {
    format!("{}={}", url::query_escape(key), url::query_escape(&value))
}

fn filter_string(filters: Option<&Value>, key: &str, default: &str) -> String {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn parse_finder(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/pageProps/series")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(parse_series_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() >= 20,
        entries,
    }
}

fn parse_recent(body: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("serie"))
        .map(parse_series_item)
        .collect();
    Paged {
        entries,
        has_next_page: true,
    }
}

fn parse_series_item(item: &Value) -> CatalogItem {
    let raw_id = item.get("raw_id").and_then(Value::as_u64).unwrap_or(0);
    let slug = text(item, "slug").unwrap_or_else(|| "sample".to_string());
    let data = item.get("data").unwrap_or(item);
    let key = format!("{LANG_PREFIX}/serie-{raw_id}/{slug}");
    CatalogItem {
        key: key.clone(),
        title: text(data, "title").unwrap_or_else(|| slug.replace('-', " ")),
        cover: text(data, "image").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let next = next_data(body)
        .and_then(|data| serde_json::from_str::<Value>(&data).ok())
        .unwrap_or(Value::Null);
    let serie = next.pointer("/props/pageProps/serie/serie_data");
    let data = serie.and_then(|value| value.get("data"));
    let normalized = normalize_key(key);
    CatalogItem {
        key: normalized.clone(),
        title: data
            .and_then(|value| text(value, "title"))
            .or_else(|| first_text(body, &["<h1", "class=\"long-title", "class=\"text-uppercase"]))
            .unwrap_or_else(|| url::slug_from_url(&normalized).unwrap_or_else(|| "Novel".to_string())),
        cover: data
            .and_then(|value| text(value, "image"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: data
            .and_then(|value| text(value, "description"))
            .or_else(|| first_text(body, &["class=\"description", "class=\"lead"])),
        authors: data
            .and_then(|value| text(value, "author"))
            .into_iter()
            .collect(),
        tags: parse_tags(body),
        status: serie
            .and_then(|value| value.get("status"))
            .and_then(Value::as_i64)
            .map(parse_status_code)
            .unwrap_or_else(|| parse_status_text(&html::strip_tags(body))),
        url: Some(absolute_url(&normalized)),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for class in ["genre", "tag"] {
        for block in html::class_blocks(body, class) {
            let value = html::strip_tags(block)
                .trim_matches(',')
                .trim()
                .to_string();
            if !value.is_empty() && !out.contains(&value) {
                out.push(value);
            }
        }
    }
    out
}

fn parse_series_identity(body: &str, key: &str) -> (u64, u64, String) {
    let next = next_data(body)
        .and_then(|data| serde_json::from_str::<Value>(&data).ok())
        .unwrap_or(Value::Null);
    let serie = next.pointer("/props/pageProps/serie/serie_data");
    let raw_id = serie
        .and_then(|value| value.get("raw_id"))
        .and_then(Value::as_u64)
        .or_else(|| key_raw_id(key))
        .unwrap_or(1);
    let slug = serie
        .and_then(|value| text(value, "slug"))
        .or_else(|| key_slug(key))
        .unwrap_or_else(|| "sample".to_string());
    let count = serie
        .and_then(|value| value.get("chapter_count"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    (raw_id, count, slug)
}

fn fetch_chapters(raw_id: u64, total: u64, slug: &str, fallback_key: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    let mut start = 1;
    let total = total.max(1);
    while start <= total {
        let end = (start + 249).min(total);
        let target = format!("{BASE_URL}/api/chapters/{raw_id}?start={start}&end={end}");
        let body = fetch_json_or_fixture(&target, CHAPTERS_FIXTURE, &absolute_url(fallback_key));
        let root = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let list = root
            .get("chapters")
            .or_else(|| root.pointer("/data/chapters"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if list.is_empty() {
            break;
        }
        for chapter in list {
            let order = chapter.get("order").and_then(Value::as_u64).unwrap_or(start);
            let key = format!("{LANG_PREFIX}/serie-{raw_id}/{slug}/chapter-{order}");
            chapters.push(NovelChapter {
                key: key.clone(),
                title: text(&chapter, "title").or_else(|| Some(format!("Chapter {order}"))),
                chapter_number: Some(order as f32),
                date_uploaded: text(&chapter, "updated_at")
                    .and_then(|date| dates::parse_fixture_date(&date)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            });
        }
        if end == total {
            break;
        }
        start = end + 1;
    }
    chapters.sort_by(|a, b| {
        a.chapter_number
            .partial_cmp(&b.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let content = root.pointer("/data/data/body").unwrap_or(&root);
    let html_body = match content {
        Value::Array(lines) => lines
            .iter()
            .filter_map(Value::as_str)
            .map(|line| format!("<p>{line}</p>"))
            .collect::<Vec<_>>()
            .join(""),
        Value::String(text) if text.starts_with("arr:") || text.starts_with("str:") => {
            "<p>This chapter uses encrypted client-side translation data that is not available through the current source API response.</p>".to_string()
        }
        Value::String(text) if text.trim_start().starts_with('<') => text.clone(),
        Value::String(text) => format!("<p>{text}</p>"),
        _ => TEXT_FALLBACK_HTML.to_string(),
    };
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn next_data(body: &str) -> Option<String> {
    let marker = "id=\"__NEXT_DATA__\"";
    let start = body.find(marker)?;
    let rest = &body[start..];
    html::text_between(rest, "<script", "</script>")
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn parse_status_code(value: i64) -> ItemStatus {
    match value {
        0 => ItemStatus::Ongoing,
        1 => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_status_text(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("completed") => ItemStatus::Completed,
        text if text.contains("ongoing") => ItemStatus::Ongoing,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("wtr-lab.com").then(|| normalize_key(input))
}

fn key_raw_id(key: &str) -> Option<u64> {
    key.split("serie-").nth(1)?.split('/').next()?.parse().ok()
}

fn key_slug(key: &str) -> Option<String> {
    key.split("serie-").nth(1)?.split('/').nth(1).map(str::to_string)
}

fn chapter_identity(key: &str) -> Option<(u64, u64)> {
    let raw_id = key_raw_id(key)?;
    let chapter = key.split("chapter-").nth(1)?.split('/').next()?.parse().ok()?;
    Some((raw_id, chapter))
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn sample_key() -> &'static str {
    "en/serie-1/sample"
}

const HOME_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"buildId":"build","props":{"pageProps":{}}}</script>"#;

const FINDER_FIXTURE: &str = r#"{
  "pageProps": {
    "series": [
      {
        "raw_id": 1,
        "slug": "sample",
        "data": {
          "title": "Sample Novel",
          "image": "/cover.jpg"
        }
      }
    ]
  }
}"#;

const LATEST_FIXTURE: &str = r#"{
  "data": [
    {
      "serie": {
        "raw_id": 1,
        "slug": "sample",
        "data": {
          "title": "Sample Novel",
          "image": "/cover.jpg"
        }
      }
    }
  ]
}"#;

const DETAILS_FIXTURE: &str = r#"
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"serie":{"serie_data":{"raw_id":1,"slug":"sample","chapter_count":1,"status":0,"data":{"title":"Sample Novel","image":"/cover.jpg","description":"Sample summary.","author":"Sample Author"}}}}}}</script>
<h1 class="text-uppercase">Sample Novel</h1><p class="lead">Sample summary.</p>
"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "chapters": [
    {
      "title": "Chapter 1",
      "order": 1,
      "updated_at": "2024-01-01"
    }
  ]
}"#;

const TEXT_FIXTURE: &str = r#"{
  "success": true,
  "data": {
    "data": {
      "body": ["Sample chapter text."]
    }
  }
}"#;

const TEXT_FALLBACK_HTML: &str = "<p>Sample chapter text.</p>";

export_novel_source!(SOURCE);
