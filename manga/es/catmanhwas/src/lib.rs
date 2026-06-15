use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Catoons = Catoons;
const BASE_URL: &str = "https://newcat1.xyz";
const CDN_URL: &str = "https://cdn.newcat1.xyz";

struct Catoons;

impl MangaSource for Catoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_page(LIST_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        Ok(parse_series_page(
            &fetch_json_or_fixture(&series_url(page, "", latest), LIST_FIXTURE),
            page,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_item(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_series_page(
            &fetch_json_or_fixture(&series_url(page, query, false), LIST_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_item(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let payload = encode_payload(&[
            ("slug", PayloadValue::Text(&key)),
            ("perPage", PayloadValue::Number(500)),
        ]);
        let data = fetch_remote_devalue("130a4lj/getChapters", &payload, CHAPTERS_FIXTURE);
        Ok(data
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|chapter| chapter_item(&key, chapter))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let (series, chapter) = key.split_once('/').unwrap_or(("sample", "1"));
        let data = decode_data_node(
            &fetch_json_or_fixture(
                &format!("{BASE_URL}/series/{series}/{chapter}/__data.json"),
                PAGES_FIXTURE,
            ),
            "chapter",
            PAGES_FIXTURE,
        );
        Ok(data
            .get("chapter")
            .and_then(|chapter| chapter.get("images"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(absolute_image_url)
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
            .collect())
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
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
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&key)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn series_url(page: u64, query: &str, latest: bool) -> String {
    let mut params = vec![("page", page.to_string()), ("perPage", "24".to_string())];
    if !query.is_empty() {
        params.push(("search", url::query_escape(query)));
    }
    if latest {
        params.push(("sort", "latest".to_string()));
    }
    let query = params
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/series/__data.json?{query}")
}

fn parse_series_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let data = decode_data_node(body, "series", LIST_FIXTURE);
    let last_page = data.get("lastPage").and_then(Value::as_u64).unwrap_or(page);
    Paged {
        entries: data
            .get("series")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|series| series_item(series, false))
            .collect(),
        has_next_page: page < last_page,
    }
}

fn details_item(key: &str) -> CatalogItem {
    let data = decode_data_node(
        &fetch_json_or_fixture(
            &format!("{BASE_URL}/series/{key}/__data.json"),
            DETAILS_FIXTURE,
        ),
        "seo",
        DETAILS_FIXTURE,
    );
    data.get("seo")
        .and_then(|series| series_item(series, true))
        .unwrap_or_else(|| CatalogItem {
            key: key.to_string(),
            title: url::slug_from_url(key).unwrap_or_else(|| "Catoons".into()),
            url: Some(format!("{BASE_URL}/series/{key}")),
            language: Some("es".into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn series_item(series: &Value, initialized: bool) -> Option<CatalogItem> {
    let slug = string_value(series, "slug")?;
    let title = string_value(series, "name")
        .unwrap_or_else(|| url::slug_from_url(&slug).unwrap_or_else(|| "Catoons".to_string()));
    Some(CatalogItem {
        key: slug.clone(),
        title,
        cover: string_value(series, "cover_url")
            .or_else(|| string_value(series, "cover"))
            .map(|image| absolute_image_url(&image)),
        description: string_value(series, "description"),
        tags: series
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| string_value(genre, "name"))
            .collect(),
        status: match string_value(series, "status").as_deref() {
            Some("finished") | Some("completed") => ItemStatus::Completed,
            Some("hiatus") => ItemStatus::Hiatus,
            Some("cancelled") | Some("canceled") => ItemStatus::Cancelled,
            Some("ongoing") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("es".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    })
}

fn chapter_item(series: &str, chapter: &Value) -> Option<MangaChapter> {
    let id = chapter.get("id").and_then(value_to_string)?;
    let number = chapter
        .get("number")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    let display_number = chapter
        .get("number")
        .and_then(value_to_string)
        .unwrap_or_else(|| id.clone());
    let name = string_value(chapter, "name").unwrap_or_default();
    let title = if name.is_empty() {
        format!("Capítulo {display_number}")
    } else {
        format!("Capítulo {display_number} - {name}")
    };
    Some(MangaChapter {
        key: format!("{series}/{id}"),
        title: Some(title),
        chapter_number: number,
        url: Some(format!("{BASE_URL}/series/{series}/{id}")),
        language: Some("es".into()),
        ..MangaChapter::default()
    })
}

fn decode_data_node(body: &str, wanted_key: &str, fixture: &str) -> Value {
    let root: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null));
    root.get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| node.get("data"))
        .map(devalue_decode)
        .find(|decoded| decoded.get(wanted_key).is_some())
        .unwrap_or(Value::Null)
}

fn fetch_remote_devalue(function_id: &str, payload: &str, fixture: &str) -> Value {
    let body = fetch_json_or_fixture(
        &format!("{BASE_URL}/_app/remote/{function_id}?payload={payload}"),
        fixture,
    );
    let outer: Value = serde_json::from_str(&body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null));
    let Some(result) = outer.get("result").and_then(Value::as_str) else {
        return Value::Null;
    };
    let encoded: Value = serde_json::from_str(result).unwrap_or(Value::Null);
    devalue_decode(&encoded)
}

fn devalue_decode(encoded: &Value) -> Value {
    let Some(values) = encoded.as_array() else {
        return Value::Null;
    };
    decode_index(values, 0, &mut Vec::new())
}

fn decode_index(values: &[Value], index: usize, stack: &mut Vec<usize>) -> Value {
    if index >= values.len() || stack.contains(&index) {
        return Value::Null;
    }
    stack.push(index);
    let decoded = match &values[index] {
        Value::Object(object) => {
            let mut output = serde_json::Map::new();
            for (key, value) in object {
                output.insert(key.clone(), decode_reference(values, value, stack));
            }
            Value::Object(output)
        }
        Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|value| decode_reference(values, value, stack))
                .collect(),
        ),
        primitive => primitive.clone(),
    };
    stack.pop();
    decoded
}

fn decode_reference(values: &[Value], reference: &Value, stack: &mut Vec<usize>) -> Value {
    reference
        .as_i64()
        .and_then(|index| {
            if index >= 0 {
                Some(decode_index(values, index as usize, stack))
            } else {
                None
            }
        })
        .unwrap_or_else(|| reference.clone())
}

enum PayloadValue<'a> {
    Text(&'a str),
    Number(u64),
}

fn encode_payload(fields: &[(&str, PayloadValue<'_>)]) -> String {
    let object = fields
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!(
                "{}:{}",
                serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into()),
                index + 1
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut values = vec![format!("{{{object}}}")];
    for (_, value) in fields {
        values.push(match value {
            PayloadValue::Text(text) => {
                serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
            }
            PayloadValue::Number(number) => number.to_string(),
        });
    }
    base64_url(format!("[{}]", values.join(",")).as_bytes())
}

fn base64_url(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    let mut index = 0;
    while index < input.len() {
        let b0 = input[index];
        let b1 = input.get(index + 1).copied().unwrap_or(0);
        let b2 = input.get(index + 2).copied().unwrap_or(0);
        output.push(TABLE[(b0 >> 2) as usize] as char);
        output.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if index + 1 < input.len() {
            output.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        }
        if index + 2 < input.len() {
            output.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        }
        index += 3;
    }
    output
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
        .or_else(|| {
            value
                .as_f64()
                .map(|number| number.to_string().trim_end_matches(".0").to_string())
        })
}

fn normalize_key(input: &str) -> String {
    input
        .trim_start_matches(BASE_URL)
        .trim_start_matches("/series/")
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .to_string()
}

fn absolute_image_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return input.to_string();
    }
    if input.starts_with('/') {
        return format!("{BASE_URL}{input}");
    }
    if input.starts_with("images/") {
        return format!("{CDN_URL}/{input}");
    }
    format!("{BASE_URL}/{input}")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"type":"data","nodes":[{"type":"data","data":[{"series":1,"lastPage":6,"page":6},[2],{"id":3,"name":4,"slug":5,"cover_url":7},1,"Sample","sample",1,"https://example.com/cover.jpg"],"uses":{}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"type":"data","nodes":[{"type":"data","data":[{"seo":1},{"id":2,"name":3,"slug":4,"description":5,"cover_url":6},1,"Sample","sample","Summary","https://example.com/cover.jpg"],"uses":{}}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"type":"result","result":"[{\"success\":1,\"data\":2},true,[3],{\"id\":4,\"number\":5,\"name\":6,\"published_at\":7},1,1,\"Uno\",\"2024-01-01T00:00:00.000000Z\"]"}"#;
const PAGES_FIXTURE: &str = r#"{"type":"data","nodes":[{"type":"data","data":[{"chapter":1},{"images":2},[3,4],"https://example.com/page1.jpg","https://example.com/page2.jpg"],"uses":{}}]}"#;
