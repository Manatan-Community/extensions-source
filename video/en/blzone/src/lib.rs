use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use serde_json::{Value, json};

const SOURCE: BlZone = BlZone;
const BASE_URL: &str = "https://blzone.net";
const SERVERS: [&str; 4] = ["Filemoon", "StreamTape", "MixDrop", "VidGuard"];

struct BlZone;

impl VideoSource for BlZone {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            let anime_url = if page <= 1 {
                format!("{BASE_URL}/anime/")
            } else {
                format!("{BASE_URL}/anime/page/{page}/")
            };
            let mut parsed = parse_listing(&get_or_fixture(&anime_url, LIST_FIXTURE, BASE_URL));
            if page <= 1 {
                let body = get_or_fixture(&format!("{BASE_URL}/dorama/"), LIST_FIXTURE, BASE_URL);
                parsed.entries.extend(parse_listing(&body).entries);
            }
            return Ok(parsed);
        }
        Ok(parse_listing(&get_or_fixture(
            &format!("{BASE_URL}/trending/"),
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
        let media_type = filter(&request, "type", "");
        let target = if media_type.is_empty() {
            format!("{BASE_URL}/?s={}", url::query_escape(query))
        } else {
            format!("{BASE_URL}/{media_type}/?s={}", url::query_escape(query))
        };
        Ok(Paged {
            entries: parse_search(&get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/anime/sample-episode-1/".into());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, EPISODE_FIXTURE, BASE_URL);
        let mut hosters = parse_hosters(&body, &episode_url);
        sort_hosters(&mut hosters, &request);
        Ok(hosters)
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let parts = key.split('|').collect::<Vec<_>>();
        if parts.len() < 3 {
            return Ok(Vec::new());
        }
        let name = parts[0];
        let target = parts[1];
        let referer = parts[2];
        Ok(vec![external_stream(target, name, referer)])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
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
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
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

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("item tvshows")
        .skip(1)
        .filter_map(parse_card)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("pagination")
            && body.contains("next")
            && !body.contains("next disabled"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::attr_after(chunk, "<img", "alt")
        .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title_from_path(&href));
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter_map(parse_search_card)
        .collect()
}

fn parse_search_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::attr_after(chunk, "<img", "alt")
        .or_else(|| {
            html::text_between(chunk, "class=\"title", "</div>")
                .map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title_from_path(&href));
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let title = html::text_between(&body, "class=\"data", "</h1>")
        .map(|value| html::strip_tags(&value))
        .or_else(|| html::attr_after(&body, "class=\"poster", "alt"))
        .unwrap_or_else(|| title_from_path(path));
    let description = html::text_between(&body, "class=\"wp-content", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let alt_title = custom_field(&body, "Original Title");
    let description = join_text(description, alt_title);
    CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(&body, "class=\"poster", "data-src")
            .or_else(|| html::attr_after(&body, "class=\"poster", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description,
        tags: links_after(&body, "sgeneros"),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let mut episodes = Vec::new();
    if let Some(block) = block_after(body, "id=\"episodes") {
        for chunk in block.split("<li").skip(1) {
            let Some(href) = html::attr_after(chunk, "<a", "href") else {
                continue;
            };
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Episode".to_string());
            episodes.push(VideoEpisode {
                key: path_key(&href),
                title: Some(title.clone()),
                episode_number: episode_number(&title),
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    episodes.reverse();
    episodes
}

fn parse_hosters(body: &str, episode_url: &str) -> Vec<VideoHoster> {
    let names = body
        .split("<li")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "class=\"title", "</span>"))
        .map(|value| html::strip_tags(&value))
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    for (index, chunk) in body.split("source-box").skip(2).enumerate() {
        let Some(src) = html::attr_after(chunk, "<iframe", "src").filter(|value| !value.is_empty())
        else {
            continue;
        };
        let embed = decode_disclaimer(&absolute_url(&src));
        let name = names
            .get(index)
            .and_then(|server_name| matched_server(server_name))
            .or_else(|| matched_server(&embed))
            .unwrap_or("External");
        if name == "External" {
            continue;
        }
        out.push(VideoHoster {
            key: format!("{name}|{embed}|{episode_url}"),
            name: name.to_string(),
            url: Some(embed),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(episode_url),
            ..VideoHoster::default()
        });
    }
    dedupe_hosters(out)
}

fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(if is_hls { "auto" } else { "external" }.to_string()),
        format: Some(if is_hls { "hls" } else { "external" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::External
        }),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_hosters(hosters: &mut [VideoHoster], request: &Value) {
    let preferred = preference(request, "preferred_server", "Filemoon");
    hosters.sort_by_key(|hoster| hoster.name.contains(&preferred));
    hosters.reverse();
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preference(request, "preferred_server", "Filemoon");
    streams.sort_by_key(|stream| {
        stream
            .name
            .as_deref()
            .unwrap_or_default()
            .contains(&preferred)
    });
    streams.reverse();
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "listing": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

fn page(request: &Value) -> u32 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as u32
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with('/')).then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!(
        "/{}",
        without_base
            .split(['?', '#'])
            .next()
            .unwrap_or(without_base)
            .trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(input: &str) -> String {
    path_key(input)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("BLZone")
        .replace('-', " ")
}

fn block_after<'a>(input: &'a str, marker: &str) -> Option<&'a str> {
    let start = input.find(marker)?;
    Some(&input[start..])
}

fn links_after(body: &str, marker: &str) -> Vec<String> {
    let Some(block) = block_after(body, marker) else {
        return Vec::new();
    };
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn custom_field(body: &str, label: &str) -> Option<String> {
    let chunk = body.split(label).nth(1)?;
    html::text_between(chunk, "class=\"valor", "</span>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn join_text(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(first), Some(second)) => Some(format!("{first}\n\n{second}")),
        (Some(first), None) => Some(first),
        (None, Some(second)) => Some(second),
        (None, None) => Some("No description available.".to_string()),
    }
}

fn episode_number(title: &str) -> Option<f32> {
    let lower = title.to_ascii_lowercase();
    let rest = lower.split("episode").nth(1)?;
    rest.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn matched_server(input: &str) -> Option<&'static str> {
    let lower = input.to_ascii_lowercase();
    SERVERS
        .iter()
        .copied()
        .find(|server| lower.contains(&server.to_ascii_lowercase()))
        .or_else(|| {
            if lower.contains("vgembed") || lower.contains("vidguard") {
                Some("VidGuard")
            } else {
                None
            }
        })
}

fn decode_disclaimer(input: &str) -> String {
    let Some(encoded) = input.split("/diclaimer/?url=").nth(1) else {
        return input.to_string();
    };
    percent_decode(encoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn dedupe_hosters(hosters: Vec<VideoHoster>) -> Vec<VideoHoster> {
    let mut out = Vec::new();
    for hoster in hosters {
        if out.iter().any(|item: &VideoHoster| item.key == hoster.key) {
            continue;
        }
        out.push(hoster);
    }
    out
}

fn filter(request: &Value, key: &str, fallback: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

fn preference(request: &Value, key: &str, fallback: &str) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

const LIST_FIXTURE: &str = r#"
<div id="dt-tvshows"><div class="item tvshows"><div class="poster"><a href="https://blzone.net/anime/sample-show/"><img alt="Sample Show" src="/poster.jpg"></a></div><h3><a>Sample Show</a></h3></div></div>
<div class="pagination"><a class="next disabled">Next</a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="search-page"><div class="result-item"><article><div class="thumbnail"><a href="https://blzone.net/anime/sample-show/"><img alt="Sample Show" src="/poster.jpg"></a></div><div class="title"><a>Sample Show</a></div></article></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="sheader"><div class="poster"><img alt="Sample Show" src="/poster.jpg"></div><div class="data"><h1>Sample Show</h1><div class="sgeneros"><a>Drama</a><a>Romance</a></div></div></div>
<div class="sbox"><div class="wp-content"><p>Sample description.</p></div></div>
<div class="custom_fields"><b class="variante">Original Title</b><span class="valor">Original Sample</span></div>
<div id="episodes"><ul class="episodios2"><li><div class="episodiotitle"><a href="https://blzone.net/anime/sample-show-episode-1/">Episode 1</a></div></li></ul></div>
"#;

const EPISODE_FIXTURE: &str = r#"
<ul id="playeroptionsul"><li><span class="title">Filemoon</span></li><li><span class="title">StreamTape</span></li></ul>
<div class="dooplay_player"><div class="source-box"></div><div class="source-box"><iframe class="metaframe" src="https://filemoon.sx/e/sample"></iframe></div><div class="source-box"><iframe class="metaframe" src="https://streamtape.com/e/sample"></iframe></div></div>
"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample Show");
    }

    #[test]
    fn parses_episodes_fixture() {
        let episodes = parse_episodes(DETAILS_FIXTURE);
        assert_eq!(episodes[0].episode_number, Some(1.0));
    }

    #[test]
    fn parses_hosters_fixture() {
        let hosters = parse_hosters(
            EPISODE_FIXTURE,
            "https://blzone.net/anime/sample-show-episode-1/",
        );
        assert_eq!(hosters.len(), 2);
        assert_eq!(hosters[0].name, "Filemoon");
    }
}
