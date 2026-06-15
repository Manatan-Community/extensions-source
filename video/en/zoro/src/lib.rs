use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Zoro = Zoro;
const BASE_URL: &str = "https://hianime.to";
const AJAX_ROUTE: &str = "/v2";
const HOSTERS: [&str; 4] = ["HD-1", "HD-2", "HD-3", "StreamTape"];
const TYPES: [(&str, &str); 4] = [
    ("servers-sub", "Sub"),
    ("servers-dub", "Dub"),
    ("servers-mixed", "Mixed"),
    ("servers-raw", "Raw"),
];

struct Zoro;

impl VideoSource for Zoro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let page = page(&request);
        let target = if listing == "latest" {
            format!("{BASE_URL}/recently-updated?page={page}")
        } else {
            format!("{BASE_URL}/most-popular?page={page}")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body, &request))
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

        let endpoint = if query.is_empty() { "filter" } else { "search" };
        let mut params = vec![("page".to_string(), page(&request).to_string())];
        if !query.is_empty() {
            params.push(("keyword".to_string(), query.to_string()));
        }
        for key in [
            "type", "status", "rated", "score", "season", "language", "sort", "sy", "sm", "sd",
            "ey", "em", "ed", "genres",
        ] {
            if let Some(value) = filter(&request, key).filter(|value| !value.trim().is_empty()) {
                params.push((key.to_string(), value));
            }
        }
        let query_string = params
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&");
        let target = format!("{BASE_URL}/{endpoint}?{query_string}");
        let body = fetch_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-anime-1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-anime-1".to_string());
        let id = path.rsplit('-').next().unwrap_or("1");
        let target = format!("{BASE_URL}/ajax{AJAX_ROUTE}/episode/list/{id}");
        let body = fetch_ajax_html(&target, EPISODES_FIXTURE, &absolute_url(&path));
        let mut episodes = parse_episodes(&body, &request);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/watch/sample-anime-1?ep=1".to_string());
        let id = episode
            .split("?ep=")
            .nth(1)
            .and_then(|tail| tail.split(['&', '#']).next())
            .unwrap_or("1");
        let target = format!("{BASE_URL}/ajax{AJAX_ROUTE}/episode/servers?episodeId={id}");
        let body = fetch_ajax_html(&target, HOSTERS_FIXTURE, &absolute_url(&episode));
        Ok(parse_hosters(&body, &episode, &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let parts = key.split('|').collect::<Vec<_>>();
        if parts.len() < 4 {
            return Ok(Vec::new());
        }
        let server_id = parts[0];
        let media_type = parts[1];
        let name = parts[2];
        let referer = parts[3];
        let target = format!("{BASE_URL}/ajax{AJAX_ROUTE}/episode/sources?id={server_id}");
        let body = fetch_or_fixture(&target, SOURCE_FIXTURE, referer);
        let link = serde_json::from_str::<SourceLink>(&body)
            .ok()
            .and_then(|response| response.link)
            .unwrap_or_default();
        if link.is_empty() {
            return Ok(Vec::new());
        }
        let mut streams = resolve_embed_streams(&link, media_type, name, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
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
                title: "Most Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recently Updated".to_string(),
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
        .with_referer(referer)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_ajax_html(target: &str, fixture: &str, referer: &str) -> String {
    let body = client(referer)
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str::<HtmlResponse>(&body)
        .map(|response| response.html)
        .unwrap_or(body)
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_listing(body: &str, request: &Value) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("flw-item")
            .skip(1)
            .filter_map(|chunk| parse_card(chunk, request))
            .collect(),
        has_next_page: body.contains("page-item") && body.contains("title=\"Next\""),
    }
}

fn parse_card(chunk: &str, request: &Value) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "film-detail", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let path = path_key(&href);
    let use_english = preference(request, "preferred_title_lang")
        .map(|value| value == "English")
        .unwrap_or(false);
    let title = if use_english {
        html::attr_after(chunk, "film-detail", "title")
            .or_else(|| html::attr_after(chunk, "<a", "title"))
    } else {
        html::attr_after(chunk, "film-detail", "data-jname")
            .or_else(|| html::attr_after(chunk, "<a", "data-jname"))
    }
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| title_from_path(&path));
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: html::attr_after(chunk, "film-poster", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let info = body.split("anisc-info").nth(1).unwrap_or(body);
    let title = html::text_between(body, "film-name", "</h2>")
        .or_else(|| html::text_between(body, "film-name", "</h1>"))
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| title_from_path(path));
    let mut description = String::new();
    if let Some(overview) = info_value(info, "Overview:") {
        description.push_str(&overview);
    }
    for tag in ["Aired:", "Premiered:", "Synonyms:", "Japanese:"] {
        if let Some(value) = info_value(info, tag) {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(tag);
            description.push(' ');
            description.push_str(&value);
        }
    }
    let promotions = parse_promotions(body);
    if !promotions.is_empty() {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Promotions:\n");
        description.push_str(&promotions.join("\n"));
    }
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "anisc-poster", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: info_list(info, "Genres:"),
        authors: info_value(info, "Studios:").into_iter().collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&info_value(info, "Status:").unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn info_value(info: &str, tag: &str) -> Option<String> {
    let block = info
        .split(tag)
        .nth(1)?
        .split("</div>")
        .next()
        .unwrap_or_default();
    let text = html::strip_tags(block);
    let value = text.trim_start_matches(tag).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn info_list(info: &str, tag: &str) -> Vec<String> {
    info.split(tag)
        .nth(1)
        .unwrap_or_default()
        .split("</div>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn parse_promotions(body: &str) -> Vec<String> {
    body.split("block_area-promotions-list")
        .nth(1)
        .unwrap_or_default()
        .split("screen-items")
        .skip(1)
        .flat_map(|chunk| chunk.split("data-title").skip(1))
        .filter_map(|chunk| {
            let title = html::attr(&format!("data-title{chunk}"), "data-title")?;
            let src = html::attr(chunk, "data-src")?;
            Some(format!("{title}: {src}"))
        })
        .collect()
}

fn parse_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let mark_fillers = preference_bool(request, "mark_fillers", true);
    body.split("ep-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let number = html::attr(chunk, "data-number")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            let title = html::attr(chunk, "title").unwrap_or_else(|| title_from_path(&href));
            let filler = mark_fillers && chunk.contains("ssl-item-filler");
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Ep. {}: {title}", display_number(number))),
                episode_number: Some(number),
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                is_filler: filler,
                labels: if filler {
                    vec!["Filler Episode".to_string()]
                } else {
                    Vec::new()
                },
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, episode_path: &str, request: &Value) -> Vec<VideoHoster> {
    let enabled_types = enabled_types(request);
    let enabled_hosters = enabled_hosters(request);
    let episode_url = absolute_url(episode_path);
    let mut out = Vec::new();
    for (type_id, type_name) in TYPES {
        if !enabled_types.iter().any(|value| value == type_id) {
            continue;
        }
        let raw_block = body.split(type_id).nth(1).unwrap_or_default();
        let block = raw_block.split("servers-").next().unwrap_or(raw_block);
        for chunk in block.split("div class=\"item").skip(1) {
            let Some(server_id) = html::attr(chunk, "data-id") else {
                continue;
            };
            let media_type =
                html::attr(chunk, "data-type").unwrap_or_else(|| type_name.to_lowercase());
            let name = html::strip_tags(chunk.split("</div>").next().unwrap_or_default());
            if name.is_empty()
                || !enabled_hosters
                    .iter()
                    .any(|hoster| hoster.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            out.push(VideoHoster {
                key: format!("{server_id}|{media_type}|{name}|{episode_url}"),
                name: format!("{name} - {type_name}"),
                url: Some(episode_url.clone()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(&episode_url),
                ..VideoHoster::default()
            });
        }
    }
    out
}

fn resolve_embed_streams(
    embed: &str,
    media_type: &str,
    name: &str,
    request: &Value,
) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, media_type, name, embed, Vec::new(), request);
    }
    if let Some(streams) =
        resolve_megacloud(embed, media_type, name, request).filter(|streams| !streams.is_empty())
    {
        return streams;
    }
    let body = fetch_or_fixture(embed, "", BASE_URL);
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        if src.contains(".m3u8") {
            return parse_hls(&src, media_type, name, embed, Vec::new(), request);
        }
        return vec![media_stream(
            &src,
            media_type,
            name,
            "direct",
            embed,
            Vec::new(),
            request,
        )];
    }
    vec![external_stream(embed, media_type, name, request)]
}

fn resolve_megacloud(
    embed: &str,
    media_type: &str,
    name: &str,
    request: &Value,
) -> Option<Vec<VideoStream>> {
    let host = embed.split("://").nth(1)?.split('/').next()?;
    if !host.contains("megacloud") && !host.contains("rapid-cloud") && !host.contains("vidsrc") {
        return None;
    }
    let id = embed.split("/e-1/").nth(1)?.split(['?', '&', '#']).next()?;
    let server_url = format!("https://{host}");
    let nonce_body = client(&server_url)
        .get(embed)
        .xhr()
        .referer(&server_url)
        .send_text()
        .ok()?;
    let nonce = find_nonce(&nonce_body)?;
    let sources_url = format!("{server_url}/embed-2/v3/e-1/getSources?id={id}&_k={nonce}");
    let body = client(&server_url)
        .get(&sources_url)
        .xhr()
        .referer(&server_url)
        .send_text()
        .ok()?;
    let response = serde_json::from_str::<Value>(&body).ok()?;
    let tracks = parse_tracks(&response, &server_url);
    let encrypted = response
        .get("encrypted")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut out = Vec::new();
    let Some(sources) = response.get("sources") else {
        return Some(out);
    };
    if encrypted && sources.is_string() {
        return Some(vec![external_stream(embed, media_type, name, request)]);
    }
    if let Some(items) = sources.as_array() {
        for item in items {
            if let Some(file) = item.get("file").and_then(Value::as_str) {
                if file.contains(".m3u8") {
                    out.extend(parse_hls(
                        file,
                        media_type,
                        name,
                        &server_url,
                        tracks.clone(),
                        request,
                    ));
                } else {
                    out.push(media_stream(
                        file,
                        media_type,
                        name,
                        "direct",
                        &server_url,
                        tracks.clone(),
                        request,
                    ));
                }
            }
        }
    }
    Some(out)
}

fn find_nonce(body: &str) -> Option<String> {
    for token in body.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if token.len() == 48 {
            return Some(token.to_string());
        }
    }
    let tokens = body
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() == 16)
        .take(3)
        .collect::<Vec<_>>();
    (tokens.len() == 3).then(|| tokens.join(""))
}

fn parse_tracks(response: &Value, referer: &str) -> Vec<SubtitleTrack> {
    response
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|track| {
            track
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "captions" || kind == "subtitles")
                .unwrap_or(false)
        })
        .filter_map(|track| {
            let file = track.get("file").and_then(Value::as_str)?;
            let label = track
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("Subtitle");
            Some(SubtitleTrack {
                url: absolute_or(file, referer),
                language: language_code(label),
                label: Some(label.to_string()),
                format: Some(if file.ends_with(".srt") { "srt" } else { "vtt" }.to_string()),
                headers: referer_headers(referer),
                is_default: label.eq_ignore_ascii_case("english"),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn parse_hls(
    target: &str,
    media_type: &str,
    name: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
) -> Vec<VideoStream> {
    let body = client(referer)
        .get(target)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(
            target, media_type, name, "auto", referer, subtitles, request,
        )];
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
            let stream_url = absolute_or(line.trim(), target);
            Some(media_stream(
                &stream_url,
                media_type,
                name,
                &quality,
                referer,
                subtitles.clone(),
                request,
            ))
        })
        .collect()
}

fn media_stream(
    stream_url: &str,
    media_type: &str,
    name: &str,
    quality: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality} - {media_type}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        subtitles,
        preferred: is_preferred(quality, name, media_type, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, media_type: &str, name: &str, request: &Value) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} - {media_type}")),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(BASE_URL),
        preferred: is_preferred("", name, media_type, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let quality_score = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        let preferred = i32::from(is_preferred(
            quality,
            stream.name.as_deref().unwrap_or_default(),
            stream.name.as_deref().unwrap_or_default(),
            request,
        ));
        (preferred, quality_score)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = is_preferred(
            stream.quality.as_deref().unwrap_or_default(),
            stream.name.as_deref().unwrap_or_default(),
            stream.name.as_deref().unwrap_or_default(),
            request,
        );
    }
}

fn is_preferred(quality: &str, server: &str, media_type: &str, request: &Value) -> bool {
    let pref_quality =
        preference(request, "preferred_quality").unwrap_or_else(|| "1080".to_string());
    let pref_server =
        preference(request, "preferred_server").unwrap_or_else(|| "HD-1".to_string());
    let pref_type = preference(request, "preferred_type").unwrap_or_else(|| "Sub".to_string());
    quality.contains(&pref_quality)
        || (server.to_lowercase().contains(&pref_server.to_lowercase())
            && media_type
                .to_lowercase()
                .contains(&pref_type.to_lowercase()))
}

fn enabled_hosters(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("hoster_selection"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| HOSTERS.iter().map(ToString::to_string).collect())
}

fn enabled_types(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("type_selection"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| TYPES.iter().map(|(id, _)| (*id).to_string()).collect())
}

fn parse_status(input: &str) -> ItemStatus {
    match input {
        "Currently Airing" => ItemStatus::Ongoing,
        "Finished Airing" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request_raw_key(request, field).map(|key| path_key(&key))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/watch/") {
        return Some(path_key(input));
    }
    None
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_base.split('#').next().unwrap_or(without_base);
    let path = if path.starts_with("/watch/") {
        path.split('&').next().unwrap_or(path)
    } else {
        path.split('?').next().unwrap_or(path)
    };
    format!("/{}", path.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn absolute_or(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        return input.to_string();
    }
    let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        input.trim_start_matches('/')
    )
}

fn title_from_path(input: &str) -> String {
    input
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("HiAnime (Dead)")
        .rsplit_once('-')
        .map(|(name, _)| name)
        .unwrap_or(input)
        .replace('-', " ")
}

fn display_number(number: f32) -> String {
    if (number.fract() - 0.0).abs() < f32::EPSILON {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn language_code(label: &str) -> Option<String> {
    let lower = label.to_lowercase();
    if lower.contains("english") {
        Some("en".to_string())
    } else {
        None
    }
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(default)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

#[derive(Deserialize)]
struct HtmlResponse {
    html: String,
}

#[derive(Deserialize)]
struct SourceLink {
    link: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<div class="flw-item"><div class="film-poster"><img data-src="/poster.jpg"></div><div class="film-detail"><a href="/watch/sample-anime-1" title="Sample Anime" data-jname="Sample Anime">Sample Anime</a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h2 class="film-name">Sample Anime</h2><div class="anisc-poster"><img src="/poster.jpg"></div><div class="anisc-info"><div class="item item-title">Overview: <span class="text">Sample overview.</span></div><div class="item item-title">Status: <span class="name">Currently Airing</span></div><div class="item-list">Genres: <a>Action</a></div><div class="item item-title">Studios: <span class="name">Sample Studio</span></div></div>
"#;

const EPISODES_FIXTURE: &str = r#"
<a class="ep-item" href="/watch/sample-anime-1?ep=1" data-number="1" title="First Episode"></a>
"#;

const HOSTERS_FIXTURE: &str = r#"
<div class="servers-sub"><div class="item" data-id="1" data-type="sub">HD-1</div><div class="item" data-id="2" data-type="sub">HD-2</div><div class="item" data-id="3" data-type="dub">HD-3</div><div class="item" data-id="4" data-type="sub">StreamTape</div></div>
"#;

const SOURCE_FIXTURE: &str = r#"{"link":"https://megacloud.tv/embed-2/v2/e-1/sample?k=1"}"#;

export_video_source!(SOURCE);
