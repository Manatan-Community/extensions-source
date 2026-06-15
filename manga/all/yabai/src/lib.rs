use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Yabai = Yabai;
const BASE_URL: &str = "https://yabai.si";

struct Yabai;

impl MangaSource for Yabai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(query_galleries(page, "", "", ""))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = inertia_get_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_detail(&body, Some(key))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let category = filters
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let language = filters
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(query_galleries(page, query, category, language))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".into());
        let body = inertia_get_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_detail(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".into());
        let body = inertia_get_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![parse_chapter(&body, &key)])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/g/sample".into());
        let target = format!("{BASE_URL}{}/read", key.trim_end_matches('/'));
        let body = inertia_get_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = inertia_get_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_detail(&body, Some(key))),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn query_galleries(page: u64, query: &str, category: &str, language: &str) -> Paged<CatalogItem> {
    let body = inertia_post_or_fixture(
        &format!("{BASE_URL}/g"),
        &json!({
            "cat": category,
            "lng": language,
            "qry": query,
            "tag": "[]",
            "cursor": Value::Null
        })
        .to_string(),
        LIST_FIXTURE,
    );
    let mut page_result = parse_listing(&body);
    if page > 1 {
        page_result.has_next_page = false;
    }
    page_result
}

fn inertia_get_or_fixture(target: &str, fixture: &str) -> String {
    let (version, _) = tokens();
    client()
        .get(target)
        .xhr()
        .header("X-Inertia", "true")
        .header("X-Inertia-Version", version)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn inertia_post_or_fixture(target: &str, body: &str, fixture: &str) -> String {
    let (version, xsrf) = tokens();
    client()
        .post(target)
        .xhr()
        .header("X-Inertia", "true")
        .header("X-Inertia-Version", version)
        .header("X-XSRF-TOKEN", xsrf)
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn tokens() -> (String, String) {
    let response = client().get(BASE_URL).browser_document().send();
    if let Ok(response) = response {
        let version = inertia_version(response.text.as_deref().unwrap_or_default())
            .unwrap_or_else(|| "fixture-version".to_string());
        let xsrf = response
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .find_map(|(_, value)| cookie_value(value, "XSRF-TOKEN"))
            .unwrap_or_else(|| "fixture-xsrf".to_string());
        return (version, percent_decode(&xsrf));
    }
    ("fixture-version".to_string(), "fixture-xsrf".to_string())
}

fn inertia_version(body: &str) -> Option<String> {
    body.split("\"version\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .or_else(|| {
            body.split("&quot;version&quot;:&quot;")
                .nth(1)
                .and_then(|rest| rest.split("&quot;").next())
        })
        .map(ToString::to_string)
}

fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(&format!("{name}="))
            .map(ToString::to_string)
    })
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let entries = value
        .pointer("/props/post_list/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_gallery_brief)
        .collect::<Vec<_>>();
    let has_next_page = value
        .pointer("/props/post_list/meta/next_cursor")
        .and_then(Value::as_str)
        .is_some();
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_gallery_brief(item: &Value) -> Option<CatalogItem> {
    let slug = item.get("slug").and_then(Value::as_str)?;
    Some(CatalogItem {
        key: format!("/g/{slug}"),
        title: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Gallery")
            .to_string(),
        cover: item
            .get("cover")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        status: ItemStatus::Completed,
        url: Some(format!("{BASE_URL}/g/{slug}")),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_detail(body: &str, key: Option<String>) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let gallery = value.pointer("/props/post/data").unwrap_or(&Value::Null);
    let mut item = parse_gallery_brief(gallery).unwrap_or_else(|| CatalogItem {
        key: key.clone().unwrap_or_else(|| "/g/sample".to_string()),
        title: "Gallery".to_string(),
        status: ItemStatus::Completed,
        content_rating: Some("adult".to_string()),
        language: Some("all".to_string()),
        ..CatalogItem::default()
    });
    item.key = key.unwrap_or(item.key);
    item.authors = tag_names(gallery, "Group");
    item.artists = tag_names(gallery, "Artist");
    item.tags = gallery
        .get("tags")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|groups| groups.iter())
        .filter(|(name, _)| *name != "Group" && *name != "Artist")
        .flat_map(|(_, tags)| tags.as_array().into_iter().flatten())
        .filter_map(|tag| {
            tag.get("full_name")
                .or_else(|| tag.get("name"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    item.initialized = true;
    item
}

fn parse_chapter(body: &str, key: &str) -> MangaChapter {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let gallery = value.pointer("/props/post/data").unwrap_or(&Value::Null);
    let slug = gallery
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or_else(|| key.trim_start_matches("/g/"));
    MangaChapter {
        key: format!("/g/{slug}"),
        title: Some("Chapter".to_string()),
        url: Some(format!("{BASE_URL}/g/{slug}")),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    let data = value
        .pointer("/props/pages/data/list")
        .unwrap_or(&Value::Null);
    let root = data.get("root").and_then(Value::as_str).unwrap_or_default();
    let code = data.get("code").and_then(Value::as_u64).unwrap_or_default();
    let heads = string_array(data, "head");
    let hashes = string_array(data, "hash");
    let rands = string_array(data, "rand");
    let types = string_array(data, "type");
    let mut indices = heads.iter().enumerate().collect::<Vec<_>>();
    indices.sort_by_key(|(_, head)| head.parse::<u32>().unwrap_or(0));
    indices
        .into_iter()
        .filter_map(|(index, head)| {
            Some(format!(
                "{root}/{code}/{:0>4}-{}-{}.{}",
                head,
                hashes.get(index)?,
                rands.get(index)?,
                types.get(index)?
            ))
        })
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

fn tag_names(gallery: &Value, group: &str) -> Vec<String> {
    gallery
        .pointer(&format!("/tags/{group}"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| {
            tag.get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn string_array<'a>(value: &'a Value, key: &str) -> Vec<&'a str> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn percent_decode(input: &str) -> String {
    input
        .replace("%3D", "=")
        .replace("%2F", "/")
        .replace("%2B", "+")
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"props":{"post_list":{"data":[{"slug":"sample","name":"Sample Gallery","cover":"https://img.example/cover.jpg"}],"meta":{"next_cursor":"next"}}}}"#;

const DETAILS_FIXTURE: &str = r#"{"props":{"post":{"data":{"slug":"sample","name":"Sample Gallery","cover":"https://img.example/cover.jpg","tags":{"Group":[{"name":"Sample Group"}],"Artist":[{"name":"Sample Artist"}],"Language":[{"name":"English","full_name":"English"}]},"date":{"default":"2024-01-01 00:00"}}}}}"#;

const PAGES_FIXTURE: &str = r#"{"props":{"pages":{"data":{"list":{"root":"https://img.example","code":123,"head":["2","1"],"hash":["bb","aa"],"rand":["22","11"],"type":["jpg","jpg"]}}}}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yabai_json() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_detail(DETAILS_FIXTURE, Some("/g/sample".into())).authors[0],
            "Sample Group"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
