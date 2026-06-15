use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{dates, html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: NovelBuddy = NovelBuddy;
const BASE_URL: &str = "https://novelbuddy.com";
const API_URL: &str = "https://api.novelbuddy.com";

struct NovelBuddy;

impl NovelSource for NovelBuddy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = lnreader::fetch_json(BASE_URL, &search_url(&request, "", page), SEARCH_FIXTURE);
        let entries = parse_api_items(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body =
            lnreader::fetch_json(BASE_URL, &search_url(&request, query, page), SEARCH_FIXTURE);
        let entries = parse_api_items(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), DETAILS_FIXTURE);
        let Some(data) = lnreader::script_json(&body, "__NEXT_DATA__") else {
            return Ok(Vec::new());
        };
        Ok(chapters_from_next_data(&data, &key))
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
            .unwrap_or_else(|| "novel/sample/chapter-1?id=1&chapterId=1".to_string());
        let content = fetch_chapter_content(&key);
        let normalized = novel::normalize_reader_html(&clean_chapter_content(&content));
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(absolute_url(&key)),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "search".to_string(),
            title: "Titles".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = lnreader::key_from_url(BASE_URL, input) {
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

fn search_url(request: &Value, query: &str, page: u64) -> String {
    let mut params = Vec::new();
    let (mut include, mut exclude) = lnreader::filter_include_exclude(request, "genre");
    include.extend(lnreader::filter_array(request, "genreInclude"));
    exclude.extend(lnreader::filter_array(request, "genreExclude"));
    if !include.is_empty() {
        params.push(("genres", include.join(",")));
    }
    if !exclude.is_empty() {
        params.push(("exclude", exclude.join(",")));
    }
    for key in ["min_ch", "max_ch"] {
        if let Some(value) = lnreader::filter_string_opt(request, key).and_then(valid_count) {
            params.push((key, value));
        }
    }
    let status = lnreader::filter_string(request, "status", "all");
    if status != "all" {
        params.push(("status", status));
    }
    let demo = lnreader::filter_array(request, "demo");
    if !demo.is_empty() {
        params.push(("demographic", demo.join(",")));
    }
    params.push(("sort", lnreader::filter_string(request, "orderBy", "views")));
    params.push(("page", page.to_string()));
    params.push(("limit", "24".to_string()));
    let keyword = lnreader::filter_string_opt(request, "keyword")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| query.to_string());
    if !keyword.is_empty() {
        params.push(("q", keyword));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{API_URL}/titles/search?{query}")
}

fn valid_count(value: String) -> Option<String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|number| *number <= 10_000)
        .map(|number| number.to_string())
}

fn parse_api_items(body: &Value) -> Vec<CatalogItem> {
    body.pointer("/data/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url_value = item.get("url")?.as_str()?;
            let key = lnreader::normalize_key(BASE_URL, url_value);
            Some(CatalogItem {
                key: key.clone(),
                title: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled")
                    .to_string(),
                cover: item
                    .get("cover")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = lnreader::fetch_document(BASE_URL, &absolute_url(key), DETAILS_FIXTURE);
    let data = lnreader::script_json(&body, "__NEXT_DATA__")
        .unwrap_or_else(|| serde_json::from_str(NEXT_DATA_FIXTURE).unwrap_or(Value::Null));
    let manga = data
        .pointer("/props/pageProps/initialManga")
        .unwrap_or(&Value::Null);
    let normalized_key = lnreader::normalize_key(BASE_URL, key);
    CatalogItem {
        key: normalized_key.clone(),
        title: manga
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        cover: manga
            .get("cover")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: manga
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| html::strip_tags(&summary.replace("<br>", "\n"))),
        authors: names(manga.get("authors")),
        artists: names(manga.get("artists")),
        tags: names(manga.get("genres")),
        status: parse_status(
            manga
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(&normalized_key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapters_from_next_data(data: &Value, novel_key: &str) -> Vec<NovelChapter> {
    let manga = data
        .pointer("/props/pageProps/initialManga")
        .unwrap_or(&Value::Null);
    let Some(id) = manga.get("id").and_then(Value::as_str) else {
        return chapters_from_array(manga.get("chapters"), novel_key, None);
    };
    let cv = manga
        .get("content_version")
        .or_else(|| manga.get("cv"))
        .and_then(Value::as_u64)
        .map(|cv| format!("?cv={cv}"))
        .unwrap_or_default();
    let api = format!("{API_URL}/titles/{id}/chapters{cv}");
    let json = lnreader::fetch_json(BASE_URL, &api, CHAPTERS_FIXTURE);
    let chapters = if json
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        chapters_from_array(json.pointer("/data/chapters"), novel_key, Some(id))
    } else {
        chapters_from_array(manga.get("chapters"), novel_key, None)
    };
    chapters.into_iter().rev().collect()
}

fn chapters_from_array(
    value: Option<&Value>,
    _novel_key: &str,
    novel_id: Option<&str>,
) -> Vec<NovelChapter> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let raw = chapter.get("url")?.as_str()?;
            let mut key = lnreader::normalize_key(BASE_URL, raw);
            if let (Some(novel_id), Some(chapter_id)) =
                (novel_id, chapter.get("id").and_then(Value::as_str))
            {
                key.push_str(&format!("?id={novel_id}&chapterId={chapter_id}"));
            }
            Some(NovelChapter {
                key: key.clone(),
                title: chapter
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                date_uploaded: chapter
                    .get("updated_at")
                    .or_else(|| chapter.get("updatedAt"))
                    .and_then(Value::as_str)
                    .and_then(dates::parse_fixture_date),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn fetch_chapter_content(key: &str) -> String {
    let novel_id = query_value(key, "id");
    let chapter_id = query_value(key, "chapterId");
    if let (Some(novel_id), Some(chapter_id)) = (novel_id, chapter_id) {
        let api = format!("{API_URL}/titles/{novel_id}/chapters/{chapter_id}");
        let json = lnreader::fetch_json(BASE_URL, &api, CHAPTER_CONTENT_FIXTURE);
        if let Some(content) = json
            .pointer("/data/chapter/content")
            .and_then(Value::as_str)
        {
            return content.to_string();
        }
    }
    let body = lnreader::fetch_document(BASE_URL, &absolute_url(key), TEXT_FIXTURE);
    let data = lnreader::script_json(&body, "__NEXT_DATA__").unwrap_or(Value::Null);
    data.pointer("/props/pageProps/initialChapter/content")
        .and_then(Value::as_str)
        .unwrap_or(TEXT_FIXTURE)
        .to_string()
}

fn clean_chapter_content(input: &str) -> String {
    input
        .replace("Find authorized novels in Webnovel", "")
        .replace("Please click www.webnovel.com for visiting.", "")
}

fn query_value(key: &str, name: &str) -> Option<String> {
    key.split('?').nth(1)?.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        "ongoing" => ItemStatus::Ongoing,
        "hiatus" => ItemStatus::Hiatus,
        "dropped" | "cancelled" => ItemStatus::Cancelled,
        "completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const SEARCH_FIXTURE: &str =
    r#"{"data":{"items":[{"url":"/novel/sample","name":"Sample Novel","cover":"/cover.jpg"}]}}"#;
const NEXT_DATA_FIXTURE: &str = r#"{"props":{"pageProps":{"initialManga":{"id":"1","url":"/novel/sample","name":"Sample Novel","cover":"/cover.jpg","status":"ongoing","summary":"Sample summary.","authors":[{"name":"Sample Author"}],"artists":[],"genres":[{"name":"Fantasy"}],"ratingStats":{"average":4.5},"chapters":[{"id":"1","url":"/novel/sample/chapter-1","name":"Chapter 1","updatedAt":"2024-01-01"}]}}}}"#;
const DETAILS_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"initialManga":{"id":"1","url":"/novel/sample","name":"Sample Novel","cover":"/cover.jpg","status":"ongoing","summary":"Sample summary.","authors":[{"name":"Sample Author"}],"artists":[],"genres":[{"name":"Fantasy"}],"chapters":[{"id":"1","url":"/novel/sample/chapter-1","name":"Chapter 1","updatedAt":"2024-01-01"}]}}}}</script>"#;
const CHAPTERS_FIXTURE: &str = r#"{"success":true,"data":{"chapters":[{"id":"1","url":"/novel/sample/chapter-1","name":"Chapter 1","updated_at":"2024-01-01"}]}}"#;
const CHAPTER_CONTENT_FIXTURE: &str = r#"{"data":{"chapter":{"content":"<p>Sample text.</p>"}}}"#;
const TEXT_FIXTURE: &str = r#"<p>Sample text.</p>"#;

export_novel_source!(SOURCE);
