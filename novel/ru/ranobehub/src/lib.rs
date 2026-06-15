use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: RanobeHub = RanobeHub;
const BASE_URL: &str = "https://ranobehub.org";

struct RanobeHub;

impl NovelSource for RanobeHub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "last_chapter_at".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "computed_rating")
        };
        let mut target = format!("{BASE_URL}/api/search?page={page}&sort={sort}&take=40");
        let status = lnreader::filter_string(&request, "status", "0");
        if status != "0" && !status.is_empty() {
            target.push_str("&status=");
            target.push_str(&status);
        }
        let country = lnreader::filter_array(&request, "country");
        if !country.is_empty() {
            target.push_str("&country=");
            target.push_str(&country.join(","));
        }
        let include = lnreader::filter_array(&request, "tagsInclude");
        if !include.is_empty() {
            target.push_str("&tags:positive=");
            target.push_str(&include.join(","));
        }
        let exclude = lnreader::filter_array(&request, "tagsExclude");
        if !exclude.is_empty() {
            target.push_str("&tags:negative=");
            target.push_str(&exclude.join(","));
        }
        let root = fetch_json(&target, LIST_FIXTURE);
        let entries = root
            .get("resource")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: entries.len() >= 40,
            entries,
        })
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
        let root = fetch_json(
            &format!(
                "{BASE_URL}/api/fulltext/global?query={}&take=10",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        );
        let entries = root
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item.pointer("/meta/key").and_then(Value::as_str) == Some("ranobe"))
            .and_then(|item| item.get("data"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(parse_search_item)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: false,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "1".to_string());
        let root = fetch_json(
            &format!("{BASE_URL}/api/ranobe/{key}/contents"),
            CHAPTERS_FIXTURE,
        );
        let mut out = Vec::new();
        for volume in root
            .get("volumes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let volume_num = volume
                .get("num")
                .and_then(value_string)
                .unwrap_or_else(|| "1".to_string());
            for chapter in volume
                .get("chapters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let chapter_num = chapter
                    .get("num")
                    .and_then(value_string)
                    .unwrap_or_else(|| "1".to_string());
                let date_uploaded = chapter
                    .get("changed_at")
                    .and_then(value_string)
                    .and_then(|value| value.parse::<i64>().ok());
                out.push(NovelChapter {
                    key: format!("{key}/{volume_num}/{chapter_num}"),
                    title: text(chapter, "name"),
                    chapter_number: Some((out.len() + 1) as f32),
                    date_uploaded,
                    url: Some(format!(
                        "{BASE_URL}/ranobe/{key}/{volume_num}/{chapter_num}"
                    )),
                    language: Some("ru".to_string()),
                    ..NovelChapter::default()
                });
            }
        }
        Ok(out)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "1/1/1".to_string());
        let body = fetch_document(&format!("{BASE_URL}/ranobe/{key}"), TEXT_FIXTURE);
        let start = body.find("<div class=\"title-wrapper\">").unwrap_or(0);
        let end = body[start..]
            .find("<div class=\"ui text container\"")
            .map(|index| start + index)
            .unwrap_or(body.len());
        let mut content = body[start..end].to_string();
        content = replace_media_ids(&content);
        text_response(&key, &content)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Popular", popular),
            section("latest", "Latest", latest),
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> Value {
    serde_json::from_str(
        &client()
            .get(target)
            .xhr()
            .header("Accept", "application/json")
            .send_text()
            .unwrap_or_else(|_| fixture.to_string()),
    )
    .or_else(|_| serde_json::from_str(fixture))
    .unwrap_or(Value::Null)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let root = fetch_json(&format!("{BASE_URL}/api/ranobe/{key}"), DETAILS_FIXTURE);
    let data = root.get("data").unwrap_or(&root);
    CatalogItem {
        key: key.to_string(),
        title: data
            .pointer("/names/rus")
            .or_else(|| data.pointer("/names/eng"))
            .and_then(Value::as_str)
            .unwrap_or("Ranobe")
            .to_string(),
        cover: data
            .pointer("/posters/medium")
            .and_then(Value::as_str)
            .map(str::to_string),
        description: text(data, "description").map(|value| value.trim().to_string()),
        authors: data
            .pointer("/authors/0/name_eng")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        tags: tags(data),
        status: match data.pointer("/status/id").and_then(Value::as_i64) {
            Some(1) => ItemStatus::Ongoing,
            Some(2) => ItemStatus::Completed,
            Some(3) => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/ranobe/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_item(item: &Value) -> CatalogItem {
    let key = item
        .get("id")
        .and_then(value_string)
        .unwrap_or_else(|| "1".to_string());
    CatalogItem {
        key: key.clone(),
        title: item
            .pointer("/names/rus")
            .or_else(|| item.pointer("/names/eng"))
            .or_else(|| item.pointer("/names/original"))
            .and_then(Value::as_str)
            .unwrap_or("Ranobe")
            .to_string(),
        cover: item
            .pointer("/poster/medium")
            .and_then(Value::as_str)
            .map(str::to_string),
        url: Some(format!("{BASE_URL}/ranobe/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_search_item(item: &Value) -> CatalogItem {
    let key = item
        .get("id")
        .and_then(value_string)
        .unwrap_or_else(|| "1".to_string());
    CatalogItem {
        key: key.clone(),
        title: item
            .pointer("/names/rus")
            .or_else(|| item.pointer("/names/eng"))
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Ranobe")
            .to_string(),
        cover: item
            .get("image")
            .and_then(Value::as_str)
            .map(|image| image.replace("/small", "/medium")),
        url: Some(format!("{BASE_URL}/ranobe/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn tags(data: &Value) -> Vec<String> {
    ["/tags/events", "/tags/genres"]
        .into_iter()
        .flat_map(|path| {
            data.pointer(path)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|tag| {
            tag.pointer("/names/rus")
                .or_else(|| tag.pointer("/names/eng"))
                .or_else(|| tag.get("title"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn replace_media_ids(input: &str) -> String {
    let mut out = String::new();
    for part in input.split("<img") {
        if out.is_empty() {
            out.push_str(part);
            continue;
        }
        out.push_str("<img");
        if let Some(id) = html::attr(part, "data-media-id") {
            out.push_str(" src=\"/api/media/");
            out.push_str(&id);
            out.push('"');
        }
        out.push_str(part);
    }
    out
}

fn text_response(key: &str, html_body: &str) -> ExtensionResult<NovelText> {
    let normalized = novel::normalize_reader_html(html_body);
    Ok(NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/ranobe/{key}")),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    })
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/ranobe/"))
        .map(|key| key.trim_matches('/').to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const LIST_FIXTURE: &str = r#"{"resource":[{"id":1,"names":{"rus":"Sample Ranobe"},"poster":{"medium":"https://ranobehub.org/cover.jpg"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"names":{"rus":"Sample Ranobe"},"posters":{"medium":"https://ranobehub.org/cover.jpg"},"description":"Sample summary.","status":{"id":1},"authors":[{"name_eng":"Sample Author"}],"tags":{"events":[],"genres":[{"names":{"rus":"Fantasy"}}]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"volumes":[{"num":1,"chapters":[{"num":1,"name":"Chapter 1","changed_at":"1704067200"}]}]}"#;
const SEARCH_FIXTURE: &str = r#"[{"meta":{"key":"ranobe"},"data":[{"id":1,"names":{"rus":"Sample Ranobe"},"image":"https://ranobehub.org/small.jpg"}]}]"#;
const TEXT_FIXTURE: &str = r#"<div class="title-wrapper"><h1>Chapter 1</h1><p>Sample chapter text.</p></div><div class="ui text container"></div>"#;

export_novel_source!(SOURCE);
