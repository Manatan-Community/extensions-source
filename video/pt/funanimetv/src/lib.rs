use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use md5::{Digest, Md5};
use serde_json::{Map, Value};

const SOURCE: FunAnimeTv = FunAnimeTv;
const BASE_URL: &str = "https://betterclass.click";
const API_URL: &str = "https://betterclass.click/api.php";

struct FunAnimeTv;

impl VideoSource for FunAnimeTv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = api("get_home_videos", None);
        let root = parse_root(&body);
        let section = if listing(&request) == "latest" {
            latest_items(&root)
        } else {
            array_field(&root, "mostViewed")
                .into_iter()
                .map(item_from_category)
                .collect()
        };
        Ok(Paged {
            entries: section,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![details_from_key(query)],
                has_next_page: false,
            });
        }
        let mut data = Map::new();
        data.insert("search_text".to_string(), Value::String(query.to_string()));
        data.insert("title_type".to_string(), Value::Null);
        data.insert("content_type".to_string(), Value::Null);
        data.insert(
            "category".to_string(),
            filter(&request, "genre").filter(|v| !v.is_empty()).map(Value::String).unwrap_or(Value::Null),
        );
        let body = api("get_search_video", Some(Value::Object(data)));
        let root = parse_root(&body);
        Ok(Paged {
            entries: root.as_array().into_iter().flatten().map(item_from_category).collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| format!("{BASE_URL}/?cid=1"));
        Ok(details_from_key(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| format!("{BASE_URL}/?cid=1"));
        let resolved = resolve_category_key(&key);
        let cid = query_param(&resolved, "cid").unwrap_or_default();
        let tid = query_param(&resolved, "tid");
        let mut data = Map::new();
        data.insert("cat_id".to_string(), Value::String(cid));
        data.insert("page".to_string(), Value::Number(1.into()));
        if let Some(tid) = tid {
            data.insert("tid".to_string(), Value::String(tid));
        }
        let body = api("get_video_by_cat_id", Some(Value::Object(data)));
        let root = parse_root(&body);
        Ok(root
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let id = str_field(item, "id");
                let title = str_field(item, "video_title");
                let ep = str_field(item, "video_ep");
                VideoEpisode {
                    key: format!("{BASE_URL}/?id={id}"),
                    title: Some(title),
                    episode_number: ep.split_whitespace().find_map(|part| part.parse::<f32>().ok()),
                    url: Some(format!("{BASE_URL}/?id={id}")),
                    language: Some("pt-BR".to_string()),
                    ..VideoEpisode::default()
                }
            })
            .collect())
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_else(|| format!("{BASE_URL}/?id=1"));
        let id = query_param(&key, "id").unwrap_or_default();
        let mut data = Map::new();
        data.insert("video_id".to_string(), Value::String(id));
        let body = api("get_single_video", Some(Value::Object(data)));
        let root = parse_root(&body);
        let Some(item) = root.as_array().and_then(|items| items.first()) else {
            return Ok(Vec::new());
        };
        let mut streams = Vec::new();
        for (field, quality) in [
            ("video_url_sd", "360p"),
            ("video_url", "720p"),
            ("video_url_fhd", "1080p"),
        ] {
            let src = str_field(item, field);
            if src.starts_with("http") {
                streams.push(video_stream(&src, quality, &request));
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Mais vistos".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Lancamentos".to_string(),
                entries: latest.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item"))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(input)),
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

fn details_from_key(key: &str) -> CatalogItem {
    let resolved = resolve_category_key(key);
    CatalogItem {
        key: resolved.clone(),
        title: query_param(&resolved, "title")
            .unwrap_or_else(|| query_param(&resolved, "cid").map(|cid| format!("Fun Anime TV #{cid}")).unwrap_or_else(|| "Fun Anime TV".to_string())),
        url: Some(resolved),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn resolve_category_key(key: &str) -> String {
    if query_param(key, "cid").is_some() {
        return key.to_string();
    }
    let Some(id) = query_param(key, "id") else {
        return key.to_string();
    };
    let mut data = Map::new();
    data.insert("video_id".to_string(), Value::String(id));
    let body = api("get_single_video", Some(Value::Object(data)));
    let root = parse_root(&body);
    let Some(item) = root.as_array().and_then(|items| items.first()) else {
        return key.to_string();
    };
    let cid = str_field(item, "cat_id");
    let tid = str_field(item, "temp_id");
    if tid.is_empty() {
        format!("{BASE_URL}/?cid={cid}")
    } else {
        format!("{BASE_URL}/?cid={cid}&tid={tid}")
    }
}

fn latest_items(root: &Value) -> Vec<CatalogItem> {
    let categories = array_field(root, "all_video_cat");
    let mut out = Vec::new();
    for field in ["latest_video", "latest_video_dub"] {
        for item in array_field(root, field) {
            let category_name = str_field(item, "category_name");
            let category = categories
                .iter()
                .find(|cat| str_field(cat, "category_name") == category_name);
            out.push(match category {
                Some(cat) => item_from_category(cat),
                None => {
                    let id = str_field(item, "id");
                    CatalogItem {
                        key: format!("{BASE_URL}/?id={id}"),
                        title: category_name,
                        cover: Some(str_field(item, "video_thumbnail_b")),
                        description: Some(str_field(item, "video_title")),
                        url: Some(format!("{BASE_URL}/?id={id}")),
                        language: Some("pt-BR".to_string()),
                        content_rating: Some("safe".to_string()),
                        initialized: true,
                        ..CatalogItem::default()
                    }
                }
            });
        }
    }
    out
}

fn item_from_category(item: &Value) -> CatalogItem {
    let cid = str_field(item, "cid").if_empty(&str_field(item, "cat_id"));
    let tid = str_field(item, "tid").if_empty(&str_field(item, "temp_id"));
    let title = str_field(item, "category_name").if_empty(&str_field(item, "temp_name"));
    let key = if tid.is_empty() {
        format!("{BASE_URL}/?cid={cid}&title={}", url::query_escape(&title))
    } else {
        format!("{BASE_URL}/?cid={cid}&tid={tid}&title={}", url::query_escape(&title))
    };
    CatalogItem {
        key: key.clone(),
        title,
        cover: Some(str_field(item, "category_image").if_empty(&str_field(item, "temp_image"))),
        description: Some(str_field(item, "sinopse")),
        tags: split_tags(&str_field(item, "genero")),
        url: Some(key),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn api(method: &str, extra: Option<Value>) -> String {
    let constants = constants();
    client()
        .post(if method == "get_app_details" { format!("{BASE_URL}/valid_g.php") } else { API_URL.to_string() })
        .form(&[("data", request_data(method, extra, &constants.sign_salt).as_str())])
        .send_text()
        .unwrap_or_else(|_| "{}".to_string())
}

fn constants() -> Constants {
    let body = client()
        .post(format!("{BASE_URL}/valid_g.php"))
        .form(&[("data", request_data("get_app_details", None, "JbWIGaSQOVoJLYCF0RU").as_str())])
        .send_text()
        .unwrap_or_default();
    let root = parse_root_key(&body, "FUN_ANIME_01");
    let item = root.as_array().and_then(|items| items.first()).cloned().unwrap_or(Value::Null);
    Constants {
        sign_salt: str_field(&item, "singsalt").if_empty("JbWIGaSQOVoJLYCF0RU"),
        array_key: str_field(&item, "array_padrao").if_empty("FUN_ANIME_01"),
    }
}

fn request_data(method: &str, extra: Option<Value>, sign_salt: &str) -> String {
    let salt = "1";
    let mut object = Map::new();
    object.insert("salt".to_string(), Value::String(salt.to_string()));
    object.insert("sign".to_string(), Value::String(md5_hex(&format!("{sign_salt}{salt}"))));
    object.insert("method_name".to_string(), Value::String(method.to_string()));
    if let Some(Value::Object(map)) = extra {
        object.extend(map);
    }
    STANDARD_NO_PAD.encode(Value::Object(object).to_string())
}

fn parse_root(body: &str) -> Value {
    let constants = constants();
    parse_root_key(body, &constants.array_key)
}

fn parse_root_key(body: &str, key: &str) -> Value {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value
        .get(key)
        .cloned()
        .or_else(|| value.get("result").cloned())
        .or_else(|| value.get("data").cloned())
        .or_else(|| value.as_object().and_then(|map| map.values().find(|v| v.is_array() || v.is_object()).cloned()))
        .unwrap_or(Value::Null)
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Dalvik/2.1.0 (Linux; U; Android 16; M2007J20CG Build/BP3A.250905.014)")
        .with_cookies_for(BASE_URL)
}

fn video_stream(src: &str, quality: &str, request: &Value) -> VideoStream {
    VideoStream {
        url: src.to_string(),
        name: Some(quality.to_string()),
        quality: Some(quality.to_string()),
        format: Some(if src.contains(".m3u8") { "hls" } else { "mp4" }.to_string()),
        is_hls: src.contains(".m3u8"),
        stream_kind: Some(if src.contains(".m3u8") { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
        headers: referer_headers(BASE_URL),
        preferred: quality == preference(request, "preferred_quality", "1080p"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "preferred_quality", "1080p");
    streams.sort_by_key(|stream| stream.quality.as_deref() == Some(&quality));
    streams.reverse();
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", Md5::digest(input.as_bytes()))
}

fn array_field<'a>(root: &'a Value, key: &str) -> Vec<&'a Value> {
    root.get(key).and_then(Value::as_array).map(|v| v.iter().collect()).unwrap_or_default()
}

fn str_field(item: &Value, key: &str) -> String {
    item.get(key)
        .or_else(|| item.get(to_camel(key).as_str()))
        .and_then(|v| v.as_str().or_else(|| v.as_i64().map(|_| "").or_else(|| v.as_u64().map(|_| ""))))
        .map(ToString::to_string)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| item.get(key).and_then(Value::as_u64).map(|v| v.to_string()).or_else(|| item.get(key).and_then(Value::as_i64).map(|v| v.to_string())).unwrap_or_default())
}

fn to_camel(input: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in input.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn split_tags(input: &str) -> Vec<String> {
    input.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str()))
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn listing(request: &Value) -> &str {
    request.get("listing").or_else(|| request.get("listingId")).and_then(Value::as_str).unwrap_or("popular")
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(request: &Value, value: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(value.to_string()));
    }
    cloned
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() { fallback.to_string() } else { self }
    }
}

struct Constants {
    sign_salt: String,
    array_key: String,
}

export_video_source!(SOURCE);
