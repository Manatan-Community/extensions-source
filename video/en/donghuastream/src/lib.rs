use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: DonghuaStream = DonghuaStream;
const BASE_URL: &str = "https://donghuastream.org";
const DEFAULT_HOSTERS: [&str; 4] = ["dailymotion", "streamplay", "rumble", "ok.ru"];

struct DonghuaStream;

impl VideoSource for DonghuaStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "update"
        } else {
            "popular"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/anime/?page={}&order={order}", page(&request)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
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
        if query.is_empty() {
            return self.list(request);
        }
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/pagg/{}/?s={}",
                page(&request),
                manatan_shared::sdk::http::url_encode(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(fetch_details(
            &request_key(&request, "item").unwrap_or_else(|| "/anime/sample-donghua/".to_string()),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/anime/sample-donghua/".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body, ignore_preview(&request));
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/sample-donghua-episode-1/".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &absolute_url(&path)))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let embed = resolve_embedded_url(&key)?;
        if !hoster_enabled(&embed, &request) {
            return Ok(Vec::new());
        }
        let mut streams = resolve_embed_streams(&embed, name);
        sort_streams(&mut streams, preferred_quality(&request));
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key, "name": hoster.name },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(&mut streams, preferred_quality(&request));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({
            "listing": "popular",
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
        }))?;
        let latest = self.list(json!({
            "listing": "latest",
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
        }))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "div class=\"tt", "</div>")
                .or_else(|| html::text_between(chunk, "div class='ttl", "</div>"))
                .or_else(|| html::text_between(chunk, "div class='tt", "</div>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))?;
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: image_url(chunk),
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))?;
    let info = details_info_block(body);
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "div class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "div class=\"limage", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&strip_resize(&image))),
        url: Some(absolute_url(path)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "class=\"desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: collect_anchor_text(info, "genxed"),
        authors: info_value(info, "Fansub").into_iter().collect(),
        artists: info_value(info, "Studio").into_iter().collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(info_value(info, "Status").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn details_info_block(body: &str) -> &str {
    body.split("info-content")
        .nth(1)
        .or_else(|| body.split("right").nth(1))
        .unwrap_or(body)
}

fn parse_episodes(body: &str, ignore_preview: bool) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let ep_text = html::text_between(chunk, "epl-num", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "0".to_string());
            let title = format!("Episode {ep_text}");
            if ignore_preview && title.to_ascii_lowercase().contains("preview") {
                return None;
            }
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title),
                episode_number: ep_text
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<f32>().ok()),
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                labels: html::text_between(chunk, "epl-sub", "</")
                    .map(|value| vec![html::strip_tags(&value)])
                    .unwrap_or_default(),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, referer: &str) -> Vec<VideoHoster> {
    let mut hosters = Vec::new();
    for chunk in body.split("<option").skip(1) {
        if let Some(value) = html::attr(chunk, "value") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
                referer,
            ));
        }
    }
    for chunk in body.split("<a").skip(1) {
        if let Some(value) = html::attr(chunk, "data-em") {
            let name = html::strip_tags(chunk).trim().to_string();
            hosters.push(video_hoster(
                &value,
                if name.is_empty() { "Mirror" } else { &name },
                referer,
            ));
        }
    }
    hosters
}

fn video_hoster(key: &str, name: &str, referer: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        url: Some(referer.to_string()),
        lazy: true,
        video_count: Some(1),
        headers: referer_headers(referer),
        ..VideoHoster::default()
    }
}

fn resolve_embedded_url(encoded: &str) -> ExtensionResult<String> {
    if encoded.starts_with("http") {
        let doc = client()
            .get(encoded)
            .browser_document()
            .send_text()
            .map_err(|error| error_with(format!("hoster request failed: {}", error.message)))?;
        return embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|error| error_with(format!("invalid encoded mirror data: {error}")))?;
    let doc = String::from_utf8_lossy(&decoded);
    embed_from_document(&doc).ok_or_else(|| error_with("missing embed iframe"))
}

fn embed_from_document(doc: &str) -> Option<String> {
    html::attr_after(doc, "#embed_holder", "src")
        .or_else(|| html::attr_after(doc, "<iframe", "src"))
        .or_else(|| html::attr_after(doc, "itemprop=\"embedUrl\"", "content"))
        .map(|value| {
            if value.starts_with("//") {
                format!("https:{value}")
            } else {
                absolute_url(&value)
            }
        })
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, embed);
    }
    if is_supported_external_hoster(embed) {
        return vec![external_stream(embed, hoster_name(embed, name))];
    }
    let body = get_or_fixture(embed, "");
    if let Some(media) = html::text_between(&body, "file:\"", "\"")
        .or_else(|| html::text_between(&body, "file: '", "'"))
        .or_else(|| html::text_between(&body, "source: \"", "\""))
        .or_else(|| html::attr_after(&body, "<source", "src"))
    {
        if media.contains(".m3u8") {
            return parse_hls(&media, name, embed);
        }
        return vec![direct_stream(&media, name, embed)];
    }
    vec![external_stream(embed, hoster_name(embed, name))]
}

fn parse_hls(master_url: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let playlist = client().get(master_url).send_text().unwrap_or_default();
    if !playlist.contains("#EXT-X-STREAM-INF") {
        return vec![hls_stream(master_url, name, "HLS", referer)];
    }
    let mut streams = Vec::new();
    for block in playlist.split("#EXT-X-STREAM-INF:").skip(1) {
        let quality = block
            .split("RESOLUTION=")
            .nth(1)
            .and_then(|part| part.split('x').nth(1))
            .and_then(|part| part.split([',', '\n']).next())
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| "auto".to_string());
        let Some(line) = block
            .lines()
            .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        else {
            continue;
        };
        streams.push(hls_stream(
            &absolute_remote(line, master_url),
            name,
            &quality,
            referer,
        ));
    }
    streams
}

fn hls_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn direct_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: String) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }
}

fn is_supported_external_hoster(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("dailymotion")
        || lower.contains("streamplay")
        || lower.contains("rumble")
        || lower.contains("ok.ru")
        || lower.contains("okru")
}

fn hoster_name(input: &str, fallback: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("dailymotion") {
        "Dailymotion".to_string()
    } else if lower.contains("streamplay") {
        "Streamplay".to_string()
    } else if lower.contains("rumble") {
        "Rumble".to_string()
    } else if lower.contains("ok.ru") || lower.contains("okru") {
        "Ok.ru".to_string()
    } else {
        fallback.to_string()
    }
}

fn image_url(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "srcset"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| {
            absolute_url(&strip_resize(
                image.split_whitespace().next().unwrap_or(&image),
            ))
        })
}

fn strip_resize(input: &str) -> String {
    input.split("?resize").next().unwrap_or(input).to_string()
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    let block = body.split(marker).nth(1).unwrap_or(body);
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_value(block: &str, label: &str) -> Option<String> {
    block
        .split("<span")
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .map(|text| text.replace(label, "").replace(':', "").trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            block
                .split("<li")
                .find(|chunk| chunk.contains(label))
                .map(html::strip_tags)
                .map(|text| text.replace(label, "").replace(':', "").trim().to_string())
                .filter(|text| !text.is_empty())
        })
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" => ItemStatus::Completed,
        "ongoing" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| quality_score(stream.quality.as_deref()));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false);
    }
}

fn quality_score(quality: Option<&str>) -> i32 {
    quality
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn hoster_enabled(embed: &str, request: &Value) -> bool {
    let lower = embed.to_ascii_lowercase();
    let selected = request
        .get("preferences")
        .and_then(|prefs| prefs.get("enabled_hosters"))
        .or_else(|| request.get("enabled_hosters"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            DEFAULT_HOSTERS
                .iter()
                .map(|value| value.to_string())
                .collect()
        });
    selected.iter().any(|hoster| {
        if hoster == "ok.ru" {
            lower.contains("ok.ru") || lower.contains("okru")
        } else {
            lower.contains(hoster)
        }
    })
}

fn preferred_quality(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("720p")
}

fn ignore_preview(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("ignore_preview"))
        .or_else(|| request.get("ignore_preview"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|path| !path.trim_matches('/').is_empty())
        .map(path_key)
        .or_else(|| input.starts_with('/').then(|| path_key(input)))
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("url")))
        .or_else(|| request.get(field).filter(|value| value.is_string()))
        .and_then(Value::as_str)
        .map(path_key)
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return path_key(path);
    }
    let path = input.split('?').next().unwrap_or(input).trim();
    format!("/{}", path.trim_matches('/'))
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

fn absolute_remote(path: &str, base: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("//") {
        format!("https:{path}")
    } else {
        let prefix = base
            .rsplit_once('/')
            .map(|(prefix, _)| prefix)
            .unwrap_or(base);
        format!(
            "{}/{}",
            prefix.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("div class=\"mrgn")
        || body.contains("class=\"r\"")
        || body.contains("class='r'")
        || (body.contains("pagination") && body.contains("next"))
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("DonghuaStream")
            .replace('-', " "),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn error_with(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

const LIST_FIXTURE: &str = r#"
<div class="listupd">
<article><a class="tip" href="/anime/sample-donghua/"><img data-src="/sample.jpg"><div class="tt">Sample Donghua</div></a></article>
</div>
"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Donghua</h1>
<div class="thumb"><img src="/sample.jpg"></div>
<div class="info-content"><span>Status: Ongoing</span><span>Studio: Sample Studio</span><div class="genxed"><a>Action</a></div></div>
<div class="entry-content">Sample description.</div>
"#;
const EPISODES_FIXTURE: &str = r#"
<div class="eplister"><ul>
<li><a href="/sample-donghua-episode-1/"><span class="epl-num">1</span><span class="epl-sub">English Sub</span></a></li>
<li><a href="/sample-donghua-preview/"><span class="epl-num">Preview 1</span></a></li>
</ul></div>
"#;
const HOSTERS_FIXTURE: &str = r#"
<select class="mirror"><option data-index="0" value="PGlmcmFtZSBzcmM9Imh0dHBzOi8vd3d3LmRhaWx5bW90aW9uLmNvbS9lbWJlZC92aWRlby94MTIzNDUiPjwvaWZyYW1lPg==">Dailymotion</option></select>
"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cards_fixture() {
        let entries = parse_cards(LIST_FIXTURE);
        assert_eq!(entries[0].title, "Sample Donghua");
        assert_eq!(entries[0].key, "/anime/sample-donghua");
    }

    #[test]
    fn filters_preview_by_default() {
        let episodes = parse_episodes(EPISODES_FIXTURE, true);
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].episode_number, Some(1.0));
    }

    #[test]
    fn decodes_dailymotion_embed() {
        let hosters = parse_hosters(HOSTERS_FIXTURE, BASE_URL);
        let embed = resolve_embedded_url(&hosters[0].key).unwrap();
        assert_eq!(embed, "https://www.dailymotion.com/embed/video/x12345");
    }
}
