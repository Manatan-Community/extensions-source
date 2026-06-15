use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Gnula = Gnula;
const BASE_URL: &str = "https://gnula.life";

struct Gnula;

impl VideoSource for Gnula {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let p = page(&request);
        let path = if listing(&request) == "latest" {
            "archives/movies/releases"
        } else {
            "archives/movies"
        };
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/{path}/page/{p}"),
            LIST_FIXTURE,
            BASE_URL,
        )))
    }
    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        let p = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search?q={}&p={p}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|v| !v.is_empty()) {
            format!("{BASE_URL}/{genre}/page/{p}")
        } else {
            format!("{BASE_URL}/archives/movies/page/{p}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }
    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        Ok(fetch_details(&path))
    }
    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/movies/sample".to_string());
        let body = fetch(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        if path.contains("/movies/") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("Pelicula".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let Some(value) = next_json(&body) else {
            return Ok(Vec::new());
        };
        let seasons = value
            .pointer("/props/pageProps/post/seasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut counter = 1.0;
        for season in seasons {
            let sn = season.get("number").and_then(Value::as_i64).unwrap_or(1);
            for ep in season
                .get("episodes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let en = ep
                    .get("number")
                    .and_then(Value::as_i64)
                    .unwrap_or(counter as i64);
                let title = ep.get("title").and_then(Value::as_str).unwrap_or_default();
                let slug = ep
                    .pointer("/slug/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ss = ep
                    .pointer("/slug/season")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ee = ep
                    .pointer("/slug/episode")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let key = format!("/series/{slug}/seasons/{ss}/episodes/{ee}");
                out.push(VideoEpisode {
                    key: key.clone(),
                    title: Some(format!("T{sn} - E{en} - {title}")),
                    episode_number: Some(counter),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    ..VideoEpisode::default()
                });
                counter += 1.0;
            }
        }
        out.reverse();
        Ok(out)
    }
    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/movies/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        let Some(value) = next_json(&body) else {
            return Ok(Vec::new());
        };
        let root = if path.contains("/movies/") {
            value.pointer("/props/pageProps/post/players")
        } else {
            value.pointer("/props/pageProps/episode/players")
        };
        let mut streams = Vec::new();
        for (key, lang) in [
            ("latino", "[LAT]"),
            ("spanish", "[CAST]"),
            ("english", "[SUB]"),
        ] {
            for region in root
                .and_then(|v| v.get(key))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let gateway = region
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if gateway.is_empty() {
                    continue;
                }
                let server = region
                    .get("cyberlocker")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(embed) = gateway_embed(gateway) {
                    streams.extend(resolve_embed(
                        &embed,
                        &format!(
                            "{lang} {}",
                            if server.is_empty() {
                                host_name(&embed)
                            } else {
                                server.to_string()
                            }
                        ),
                        &referer,
                        &request,
                    ));
                }
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
                title: "Peliculas".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Estrenos".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }
    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }
    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
    }
    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path)),
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

#[derive(Debug, Deserialize)]
struct Listing {
    props: ListingProps,
}
#[derive(Debug, Deserialize)]
struct ListingProps {
    #[serde(rename = "pageProps")]
    page_props: ListingPageProps,
}
#[derive(Debug, Deserialize)]
struct ListingPageProps {
    results: Results,
}
#[derive(Debug, Deserialize)]
struct Results {
    #[serde(default, rename = "__typename")]
    typename: String,
    #[serde(default)]
    data: Vec<Entry>,
}
#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    titles: Titles,
    #[serde(default)]
    images: Images,
    #[serde(default)]
    slug: Slug,
    #[serde(default)]
    url: Slug,
}
#[derive(Debug, Default, Deserialize)]
struct Titles {
    #[serde(default)]
    name: String,
}
#[derive(Debug, Default, Deserialize)]
struct Images {
    #[serde(default)]
    poster: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
struct Slug {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
}

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}
fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let json = next_json(body).and_then(|v| serde_json::from_value::<Listing>(v).ok());
    let Some(listing) = json else {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    };
    let mut typ = listing.props.page_props.results.typename;
    let entries = listing
        .props
        .page_props
        .results
        .data
        .into_iter()
        .map(|e| {
            if !e.url.slug.is_empty() {
                typ = if e.url.slug.contains("series") {
                    "PaginatedSerie".to_string()
                } else if e.url.slug.contains("movies") {
                    "PaginatedMovie".to_string()
                } else {
                    typ.clone()
                };
            }
            let path = if typ.contains("Serie") {
                format!("/series/{}", e.slug.name)
            } else {
                format!("/movies/{}", e.slug.name)
            };
            CatalogItem {
                key: path.clone(),
                title: e.titles.name,
                cover: e.images.poster.map(|v| v.replace("/original/", "/w200/")),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("page-item active")
            && body
                .split("page-item active")
                .nth(1)
                .is_some_and(|tail| tail.contains("page-item")),
    }
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let value = next_json(&body).unwrap_or(Value::Null);
    let post = value.pointer("/props/pageProps/post");
    CatalogItem {
        key: path_key(path),
        title: post
            .and_then(|v| v.pointer("/titles/name"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| title_from_path(path)),
        cover: post
            .and_then(|v| v.pointer("/images/poster"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(absolute_url(path)),
        description: post
            .and_then(|v| v.get("overview"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: post
            .and_then(|v| v.get("genres"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|g| {
                        g.get("name")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if path.contains("/movies/") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: true,
        ..CatalogItem::default()
    }
}
fn gateway_embed(url: &str) -> Option<String> {
    let body = client(url)
        .get(url)
        .browser_document()
        .referer(BASE_URL)
        .send_text()
        .ok()?;
    Regex::new(r#"var url = '([^']+)'"#)
        .ok()?
        .captures(&body)?
        .get(1)
        .map(|m| m.as_str().to_string())
}
fn next_json(body: &str) -> Option<Value> {
    body.split("<script")
        .filter(|script| {
            script.contains(r#""props":{"pageProps":"#)
                || script.contains(r#"{\"props\":{\"pageProps\":"#)
        })
        .filter_map(|script| {
            script
                .split_once('>')
                .and_then(|(_, rest)| rest.split_once("</script").map(|(data, _)| data.trim()))
        })
        .find_map(|data| serde_json::from_str(data).ok())
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(media) = first_media_url(&body).map(|v| absolute_remote(&v, embed)) {
        if media.contains(".m3u8") {
            parse_hls(&media, name, embed, request)
        } else {
            vec![stream(&media, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)"#,
        r#"src\s*:\s*["']([^"']+)"#,
        r#"<source[^>]+src=["']([^"']+)"#,
        r#"url\s*=\s*["']([^"']+)"#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}
fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut quality = pref(request, "preferred_quality", "auto");
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
                true,
            ));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, &quality, referer, true));
    }
    out
}
fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}
fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} External")),
        quality: Some(name.to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let lang = pref(request, "preferred_language", "[LAT]");
    let server = pref(request, "preferred_server", "Voe").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let n = s.name.clone().unwrap_or_default();
        (
            n.contains(&lang),
            n.to_ascii_lowercase().contains(&server),
            n.contains(&quality),
            quality_rank(&n),
        )
    });
    streams.reverse();
}
fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
    }
}
fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|p| p.starts_with('/'))
        .map(path_key)
}
fn path_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}
fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}
fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}
fn listing(request: &Value) -> String {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
        .to_string()
}
fn with_listing(request: &Value, value: &str) -> Value {
    let mut next = request.clone();
    if let Some(map) = next.as_object_mut() {
        map.insert("listing".to_string(), Value::String(value.to_string()));
    }
    next
}
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}
fn quality_rank(input: &str) -> i32 {
    input
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
        .unwrap_or(0)
}
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Gnula")
        .replace('-', " ")
}
fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"results":{"__typename":"PaginatedMovie","data":[{"titles":{"name":"Sample"},"images":{"poster":"/w200/sample.jpg"},"slug":{"name":"sample"},"url":{"slug":"movies"}}]}}}}</script>"#;
const DETAILS_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"post":{"titles":{"name":"Sample"},"images":{"poster":"/sample.jpg"},"overview":"Sample description.","genres":[{"name":"Drama"}],"players":{"latino":[],"spanish":[],"english":[]},"seasons":[]}}},"page":"/movies/[slug]"}</script>"#;
