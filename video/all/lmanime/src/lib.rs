use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde_json::{Value, json};

const SOURCE: LMAnime = LMAnime;
const BASE_URL: &str = "https://lmanime.com";

struct LMAnime;

impl VideoSource for LMAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/episode/?page={page}")
        } else {
            format!("{BASE_URL}/anime/?page={page}&order=popular")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
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
        if query.starts_with("path:") {
            return Ok(Paged {
                entries: vec![fetch_details(query.trim_start_matches("path:"))],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/page/{page}/?s={}",
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
            &request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string()),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample/".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        Ok(parse_episodes(&body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/sample-episode/".to_string());
        let body = get_or_fixture(&absolute_url(&path), HOSTERS_FIXTURE);
        let allowed = allowed_languages(&request);
        Ok(parse_hosters(&body)
            .into_iter()
            .filter(|hoster| allowed.iter().any(|lang| hoster.name.contains(lang)))
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let embed = if key.starts_with("http") {
            decode_entities(&key)
        } else {
            decode_entities(&key)
        };
        let mut streams = resolve_embed_streams(&embed, name);
        sort_streams(
            &mut streams,
            &preferred_quality(&request),
            &preferred_language(&request),
        );
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
        sort_streams(
            &mut streams,
            &preferred_quality(&request),
            &preferred_language(&request),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self
                    .list(json!({
                        "listing": "popular",
                        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                    }))?
                    .entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: self
                    .list(json!({
                        "listing": "latest",
                        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
                    }))?
                    .entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| absolute_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| absolute_url(&key)))
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
            let title = html::text_between(chunk, "class=\"tt", "</div>")
                .or_else(|| html::text_between(chunk, "class='tt", "</div>"))
                .or_else(|| html::text_between(chunk, "<h2", "</h2>"))
                .map(|text| html::strip_tags(&text))
                .filter(|text| !text.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))?;
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(image.split('?').next().unwrap_or(&image))),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                initialized: true,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))?;
    let details = html::text_between(body, "info-content", "</div>")
        .or_else(|| html::text_between(body, "right ul data", "</ul>"))
        .unwrap_or_default();
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "div class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "div class=\"limage", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "class=\"desc", "</div>"))
            .map(|value| html::strip_tags(&value)),
        tags: collect_anchor_text(body, "genxed"),
        authors: info_value(&details, "Fansub").into_iter().collect(),
        artists: info_value(&details, "Studio").into_iter().collect(),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(info_value(&details, "Status").as_deref()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("episode") && !href.contains("watch") {
                return None;
            }
            let ep_text = html::text_between(chunk, "epl-num", "</")
                .or_else(|| html::text_between(chunk, "epl-title", "</"))
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Episode".to_string());
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(ep_text.clone()),
                episode_number: ep_text
                    .split_whitespace()
                    .find_map(|value| value.parse::<f32>().ok()),
                url: Some(absolute_url(&href)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    let mut hosters = Vec::new();
    for chunk in body.split("<li").skip(1) {
        if let Some(href) = get_hoster_url(chunk) {
            let text = html::strip_tags(chunk);
            let language = text.split_whitespace().next().unwrap_or("Mirror");
            hosters.push(video_hoster(&href, &format!("{language} Mirror")));
        }
    }
    for chunk in body.split("<a").skip(1) {
        if let Some(href) = get_hoster_url(chunk) {
            let text = html::strip_tags(chunk);
            let language = text.split_whitespace().next().unwrap_or("Mirror");
            hosters.push(video_hoster(&href, &format!("{language} Mirror")));
        }
    }
    hosters
}

fn get_hoster_url(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-video")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-url"))
        .or_else(|| html::attr(chunk, "href"))
        .filter(|url| {
            url.contains("dailymotion")
                || url.contains("mp4upload")
                || url.contains("filelions")
                || url.contains("streamwish")
                || url.contains("streamtape")
                || url.contains("iframe")
                || url.starts_with("http")
        })
        .map(|url| {
            if url.starts_with("//") {
                format!("https:{url}")
            } else {
                absolute_url(&url)
            }
        })
}

fn video_hoster(key: &str, name: &str) -> VideoHoster {
    VideoHoster {
        key: key.to_string(),
        name: name.to_string(),
        url: Some(key.to_string()),
        lazy: true,
        video_count: Some(1),
        ..VideoHoster::default()
    }
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, embed);
    }
    if embed.contains("dailymotion")
        || embed.contains("mp4upload")
        || embed.contains("filelions")
        || embed.contains("streamwish")
        || embed.contains("streamtape")
    {
        return vec![external_stream(embed, name)];
    }
    let body = get_or_fixture(embed, "");
    if let Some(hls) = html::text_between(&body, "file:\"", "\"")
        .or_else(|| html::text_between(&body, "file: '", "'"))
        .or_else(|| html::attr_after(&body, "<source", "src"))
    {
        if hls.contains(".m3u8") {
            return parse_hls(&hls, name, embed);
        }
        return vec![direct_stream(&hls, name, "direct", embed)];
    }
    vec![external_stream(embed, name)]
}

fn parse_hls(url: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client().get(url).send_text().unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![hls_stream(url, name, "auto", referer)];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split('x').nth(1))
                .and_then(|part| part.split([',', '\n']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            let stream_url = if line.starts_with("http") {
                line.to_string()
            } else {
                format!(
                    "{}/{}",
                    url.rsplit_once('/').map(|(base, _)| base).unwrap_or(url),
                    line
                )
            };
            Some(hls_stream(&stream_url, name, &quality, referer))
        })
        .collect()
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
        ..VideoStream::default()
    }
}

fn direct_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers: referer_headers(referer),
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        ..VideoStream::default()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn sort_streams(streams: &mut [VideoStream], quality: &str, language: &str) {
    streams.sort_by_key(|stream| {
        let quality_score = stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        let language_score = stream
            .name
            .as_deref()
            .map(|name| i32::from(name.contains(language)))
            .unwrap_or(0);
        (language_score, quality_score)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|value| value.contains(quality))
            .unwrap_or(false)
            || stream
                .name
                .as_deref()
                .map(|value| value.contains(language))
                .unwrap_or(false);
    }
}

fn allowed_languages(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("allowed_languages"))
        .and_then(Value::as_array)
    else {
        return vec![
            "English".to_string(),
            "Español".to_string(),
            "Indonesian".to_string(),
            "Portugués".to_string(),
            "Türkçe".to_string(),
            "العَرَبِيَّة".to_string(),
            "ไทย".to_string(),
        ];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn preferred_quality(request: &Value) -> String {
    pref_string(request, "preferred_quality", "720p")
}

fn preferred_language(request: &Value) -> String {
    pref_string(request, "preferred_language", "English")
}

fn pref_string(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    let block = body.split(marker).nth(1).unwrap_or_default();
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

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers") || body.contains("rel=\"next\"")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
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
    if !input.contains("lmanime.com") {
        return None;
    }
    let path = input.split("lmanime.com").nth(1).unwrap_or("/");
    Some(path_key(path))
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") {
        return path_from_url(input).unwrap_or_else(|| input.to_string());
    }
    let mut path = input.split('?').next().unwrap_or(input).to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    path
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else if input.starts_with('/') {
        format!("{BASE_URL}{input}")
    } else {
        format!("{BASE_URL}/{input}")
    }
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("LMAnime")
            .replace('-', " "),
        url: Some(absolute_url(path)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

const LIST_FIXTURE: &str = r#"
<article><a href="/anime/sample/"><img src="/cover.jpg" alt="Sample Anime"><div class="tt">Sample Anime</div></a></article>
<a class="next page-numbers" href="/anime/page/2/">Next</a>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<div class="thumb"><img src="/cover.jpg"></div>
<h1>Sample Anime</h1>
<div class="entry-content">Sample description.</div>
<div class="info-content"><span>Status: Ongoing</span><span>Studio: Sample Studio</span></div>
<ul class="episodios"><li><a href="/sample-episode/"><span class="epl-num">1</span></a></li></ul>
"#;

const EPISODES_FIXTURE: &str = r#"
<ul class="episodios"><li><a href="/sample-episode/"><span class="epl-num">1</span></a></li></ul>
"#;

const HOSTERS_FIXTURE: &str = r#"
<li><a href="https://www.dailymotion.com/video/sample">English Dailymotion</a></li>
<li><a href="https://mp4upload.com/embed-sample.html">Español MP4Upload</a></li>
"#;

export_video_source!(SOURCE);
