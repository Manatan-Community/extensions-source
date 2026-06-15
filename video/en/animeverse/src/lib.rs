use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionResult, system_time},
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const SOURCE: AnimeVerse = AnimeVerse;
const BASE_URL: &str = "https://animeverse.to";
const FINGERPRINT: &str = r#"{"ua":"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36","language":"en-US","timezone":"UTC","hw":8,"screen":"1920x1080x24","canvas":"kW9_MAWuv_3eBlyA7DxVWY","webgl":"Google Inc. (NVIDIA)|ANGLE (NVIDIA, GeForce GTX 1060 Direct3D11 vs_5_0 ps_5_0)"}"#;

struct AnimeVerse;

#[derive(Clone, Debug)]
struct Session {
    key: String,
    cookie: String,
}

impl VideoSource for AnimeVerse {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "/api/v1/recent".to_string()
        } else {
            format!("/api/v1/trending?period=today&page={page}")
        };
        let body = signed_get(&path).unwrap_or_else(|| {
            if listing == "latest" {
                RECENT_FIXTURE.to_string()
            } else {
                TRENDING_FIXTURE.to_string()
            }
        });
        Ok(if listing == "latest" {
            parse_recent_page(&body)
        } else {
            parse_catalog_page(&body, &request)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&slug, &request)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(json!({ "listing": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) }));
        }
        let path = format!("/api/v1/catalog?q={}", url::query_escape(query));
        let body = signed_get(&path).unwrap_or_else(|| CATALOG_FIXTURE.to_string());
        let query_lc = query.to_lowercase();
        let mut page = parse_catalog_page(&body, &request);
        page.entries.retain(|item| {
            item.title.to_lowercase().contains(&query_lc)
                || item
                    .alternate_titles
                    .iter()
                    .any(|title| title.to_lowercase().contains(&query_lc))
        });
        page.has_next_page = false;
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        Ok(fetch_details(&slug_from_key(&key), &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample".to_string());
        let slug = slug_from_key(&key);
        let body = signed_get(&format!("/api/v1/anime/{slug}")).unwrap_or_else(|| DETAILS_FIXTURE.to_string());
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_else(|| {
            URL_SAFE_NO_PAD.encode(json!({ "slug": "sample", "episode": 1 }).to_string())
        });
        let payload = decode_json_key(&key);
        let slug = payload
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("sample");
        let episode_number = payload.get("episode").and_then(Value::as_u64).unwrap_or(1);
        let session = fetch_session();
        let body = session
            .as_ref()
            .and_then(|session| signed_get_with_session(&format!("/api/v1/anime/{slug}"), session))
            .unwrap_or_else(|| DETAILS_FIXTURE.to_string());
        let streams = parse_streams(
            &body,
            slug,
            episode_number,
            session.as_ref().map(|session| session.cookie.as_str()).unwrap_or_default(),
            pref_bool(&request, "direct_mp4", false),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Trending".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| format!("{BASE_URL}/series/{}", slug_from_key(&key))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let payload = decode_json_key(&key);
        let slug = payload.get("slug").and_then(Value::as_str).unwrap_or_default();
        let episode = payload.get("episode").and_then(Value::as_u64).unwrap_or(1);
        if slug.is_empty() {
            Ok(None)
        } else {
            Ok(Some(format!("{BASE_URL}/series/{slug}/{episode}")))
        }
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug, &request)),
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

fn fetch_session() -> Option<Session> {
    let response = client()
        .post(&format!("{BASE_URL}/api/v1/session"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(format!(r#"{{"fp":{FINGERPRINT}}}"#))
        .send()
        .ok()?;
    let cookie = response
        .headers
        .iter()
        .find(|(name, value)| {
            name.eq_ignore_ascii_case("set-cookie") && value.starts_with("av_session=")
        })
        .map(|(_, value)| {
            value
                .trim_start_matches("av_session=")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default();
    let root: Value = serde_json::from_str(response.text.as_deref().unwrap_or_default()).ok()?;
    let key = root.get("clientAuthKey").and_then(Value::as_str)?.to_string();
    Some(Session { key, cookie })
}

fn signed_get(path: &str) -> Option<String> {
    let session = fetch_session()?;
    signed_get_with_session(path, &session)
}

fn signed_get_with_session(path: &str, session: &Session) -> Option<String> {
    let ts = unix_millis().to_string();
    let signature = sign_request("GET", path_without_query(path), &ts, &session.key)?;
    let http = client();
    let mut request = http
        .get(&format!("{BASE_URL}{path}"))
        .header("Accept", "application/json")
        .header("x-av-ts", &ts)
        .header("x-av-sig", &signature)
        .xhr();
    if !session.cookie.is_empty() {
        request = request.header("Cookie", &format!("av_session={}", session.cookie));
    }
    request.send_text().ok()
}

fn sign_request(method: &str, path: &str, ts: &str, key: &str) -> Option<String> {
    let key = URL_SAFE_NO_PAD.decode(key).ok()?;
    let mut mac = HmacSha256::new_from_slice(&key).ok()?;
    mac.update(format!("{method}|{path}|{ts}").as_bytes());
    let bytes = mac.finalize().into_bytes();
    Some(URL_SAFE_NO_PAD.encode(&bytes[..16]))
}

fn path_without_query(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn fetch_details(slug: &str, request: &Value) -> CatalogItem {
    let body = signed_get(&format!("/api/v1/anime/{slug}")).unwrap_or_else(|| DETAILS_FIXTURE.to_string());
    let catalog = signed_get("/api/v1/catalog").unwrap_or_else(|| CATALOG_FIXTURE.to_string());
    let mut item = parse_details(&body, &catalog, request);
    if item.key.is_empty() {
        item.key = slug.to_string();
    }
    item
}

fn parse_catalog_page(body: &str, request: &Value) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(CATALOG_FIXTURE).unwrap());
    let entries = extract_array(&root)
        .into_iter()
        .filter_map(|value| catalog_item(value, request))
        .collect();
    Paged {
        entries,
        has_next_page: root
            .get("hasNext")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_recent_page(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(RECENT_FIXTURE).unwrap());
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let slug = value.get("seriesSlug").and_then(Value::as_str)?;
            Some(CatalogItem {
                key: slug.to_string(),
                title: value
                    .get("seriesTitle")
                    .and_then(Value::as_str)
                    .unwrap_or("AnimeVerse")
                    .to_string(),
                cover: resolve_image(value.get("thumb").and_then(Value::as_str)),
                tags: value
                    .get("language")
                    .and_then(Value::as_str)
                    .map(|language| vec![language.to_uppercase()])
                    .unwrap_or_default(),
                url: Some(format!("{BASE_URL}/series/{slug}")),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_item(value: &Value, request: &Value) -> Option<CatalogItem> {
    let slug = value.get("slug").and_then(Value::as_str)?;
    let main_title = value.get("title").and_then(Value::as_str).unwrap_or("AnimeVerse");
    let alt_title = value
        .get("alternativeTitle")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty());
    let title = if pref_bool(request, "use_alt_title", false) {
        alt_title.unwrap_or(main_title)
    } else {
        main_title
    };
    Some(CatalogItem {
        key: slug.to_string(),
        title: title.to_string(),
        alternate_titles: alt_title
            .filter(|title| *title != main_title)
            .map(|title| vec![title.to_string()])
            .unwrap_or_default(),
        cover: resolve_image(
            value
                .get("cover")
                .or_else(|| value.get("thumb"))
                .and_then(Value::as_str),
        ),
        tags: string_array(value, "genres"),
        authors: string_array(value, "studios"),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, catalog_body: &str, request: &Value) -> CatalogItem {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let catalog_root: Value = serde_json::from_str(catalog_body).unwrap_or_else(|_| serde_json::from_str(CATALOG_FIXTURE).unwrap());
    let slug = root.get("slug").and_then(Value::as_str).unwrap_or("sample");
    let cat = extract_array(&catalog_root)
        .into_iter()
        .find(|value| value.get("slug").and_then(Value::as_str) == Some(slug));
    let main_title = root.get("title").and_then(Value::as_str).unwrap_or("AnimeVerse");
    let alt_title = cat
        .and_then(|value| value.get("alternativeTitle"))
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty() && *title != main_title);
    let title = if pref_bool(request, "use_alt_title", false) {
        alt_title.unwrap_or(main_title)
    } else {
        main_title
    };
    let rating = root
        .get("rating")
        .and_then(Value::as_f64)
        .filter(|rating| *rating > 0.0);
    let mut description = Vec::new();
    if let Some(rating) = rating {
        description.push(format!("Rating: {rating:.2}"));
    }
    if let Some(synopsis) = root.get("synopsis").and_then(Value::as_str) {
        description.push(html::strip_tags(synopsis));
    }
    if let Some(kind) = root.get("type").and_then(Value::as_str) {
        description.push(format!("Type: {kind}"));
    }
    if let Some(label) = root.get("ratingLabel").and_then(Value::as_str) {
        description.push(format!("Rating label: {label}"));
    }
    let episode_count = root
        .get("episodes")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if episode_count > 0 {
        description.push(format!("Episodes: {episode_count}"));
    }
    CatalogItem {
        key: slug.to_string(),
        title: title.to_string(),
        alternate_titles: alt_title.map(|title| vec![title.to_string()]).unwrap_or_default(),
        cover: resolve_image(
            root.get("cover")
                .or_else(|| root.get("thumb"))
                .and_then(Value::as_str),
        ),
        description: if description.is_empty() {
            None
        } else {
            Some(description.join("\n\n"))
        },
        rating: rating.map(|rating| (rating / 2.0) as f32),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let slug = root.get("slug").and_then(Value::as_str).unwrap_or("sample");
    let mut numbers = root
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| episode.get("number").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
        .into_iter()
        .rev()
        .map(|number| VideoEpisode {
            key: URL_SAFE_NO_PAD.encode(json!({ "slug": slug, "episode": number }).to_string()),
            title: Some(format!("Episode {number}")),
            episode_number: Some(number as f32),
            language: Some("en".to_string()),
            url: Some(format!("{BASE_URL}/series/{slug}/{number}")),
            ..VideoEpisode::default()
        })
        .collect()
}

fn parse_streams(
    body: &str,
    slug: &str,
    episode_number: u64,
    session_cookie: &str,
    prefer_direct: bool,
) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let referer = format!("{BASE_URL}/series/{slug}/{episode_number}");
    let mut out = Vec::new();
    for episode in root
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|episode| episode.get("number").and_then(Value::as_u64) == Some(episode_number))
    {
        let kind = episode
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("sub")
            .to_uppercase();
        let Some(stream_path) = episode.get("stream").and_then(Value::as_str) else {
            continue;
        };
        let direct = decode_stream_url(stream_path).map(|url| VideoStream {
            url,
            name: Some(format!("{kind} - Direct")),
            quality: Some("auto".to_string()),
            format: Some("mp4".to_string()),
            stream_kind: Some(VideoStreamKind::Direct),
            initialized: true,
            ..VideoStream::default()
        });
        let proxied = VideoStream {
            url: format!("{BASE_URL}{stream_path}"),
            name: Some(format!("{kind} - Proxied")),
            quality: Some("auto".to_string()),
            format: Some("mp4".to_string()),
            stream_kind: Some(VideoStreamKind::Direct),
            headers: stream_headers(&referer, session_cookie),
            initialized: true,
            ..VideoStream::default()
        };
        if prefer_direct {
            if let Some(stream) = direct {
                out.push(stream);
            }
            out.push(proxied);
        } else {
            out.push(proxied);
            if let Some(stream) = direct {
                out.push(stream);
            }
        }
    }
    out
}

fn stream_headers(referer: &str, cookie: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    if !cookie.is_empty() {
        headers.insert("Cookie".to_string(), format!("av_session={cookie}"));
    }
    headers
}

fn decode_stream_url(path: &str) -> Option<String> {
    let encoded = path.split("/v/").nth(1)?.split('.').next()?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    String::from_utf8(decoded).ok().filter(|url| url.starts_with("http"))
}

fn resolve_image(path: Option<&str>) -> Option<String> {
    let path = path?.trim();
    if path.is_empty() {
        return None;
    }
    if path.starts_with("http") {
        return Some(path.to_string());
    }
    if let Some(encoded) = path.strip_prefix("/i/") {
        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(encoded) {
            if let Ok(url) = String::from_utf8(decoded) {
                if url.starts_with("http") {
                    return Some(url);
                }
            }
        }
    }
    Some(format!("{BASE_URL}{path}"))
}

fn extract_array(root: &Value) -> Vec<&Value> {
    if let Some(array) = root.as_array() {
        return array.iter().collect();
    }
    root.as_object()
        .and_then(|object| object.values().find_map(Value::as_array))
        .map(|array| array.iter().collect())
        .unwrap_or_default()
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn decode_json_key(key: &str) -> Value {
    URL_SAFE_NO_PAD
        .decode(key)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split("/series/")
        .nth(1)
        .map(|value| value.split(['/', '?', '#']).next().unwrap_or(value).to_string())
}

fn slug_from_key(key: &str) -> String {
    slug_from_url(key).unwrap_or_else(|| {
        key.trim_matches('/')
            .split('/')
            .next_back()
            .unwrap_or(key)
            .to_string()
    })
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| request.get("key").and_then(Value::as_str).map(ToString::to_string))
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn unix_millis() -> u128 {
    system_time()
        .map(|time| time.unix_millis.max(0) as u128)
        .unwrap_or(0)
}

const TRENDING_FIXTURE: &str = r#"{"items":[{"slug":"sample","title":"Sample AnimeVerse","alternativeTitle":"Sample AV","cover":"/i/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL2NvdmVyLmpwZw","thumb":null,"genres":["Action"],"studios":["Fixture Studio"]}],"hasNext":false}"#;
const CATALOG_FIXTURE: &str = r#"[{"slug":"sample","title":"Sample AnimeVerse","alternativeTitle":"Sample AV","cover":"/i/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL2NvdmVyLmpwZw","thumb":null,"genres":["Action"],"studios":["Fixture Studio"],"searchTitle":"sample animeverse"}]"#;
const RECENT_FIXTURE: &str = r#"{"items":[{"seriesSlug":"sample","seriesTitle":"Sample AnimeVerse","thumb":"/i/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL2NvdmVyLmpwZw","language":"sub"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"slug":"sample","title":"Sample AnimeVerse","cover":"/i/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL2NvdmVyLmpwZw","synopsis":"Fixture synopsis.","type":"TV","rating":8.2,"ratingLabel":"PG-13","episodes":[{"number":1,"kind":"sub","stream":"/v/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL3ZpZGVvLm1wNA.mp4"},{"number":1,"kind":"dub","stream":"/v/aHR0cHM6Ly9maXh0dXJlcy5pbnZhbGlkL2R1Yi5tcDQ.mp4"}]}"#;

export_video_source!(SOURCE);
