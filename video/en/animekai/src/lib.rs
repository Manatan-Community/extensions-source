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

const SOURCE: AnimeKai = AnimeKai;
const BASE_URL: &str = "https://animekai.to";
const HOSTERS: [&str; 2] = ["Server 1", "Server 2"];
struct AnimeKai;

impl VideoSource for AnimeKai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let path = if listing(&request) == "latest" {
            "updates"
        } else {
            "trending"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/{path}?page={page}"),
            LIST_FIXTURE,
            BASE_URL,
        );
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
        if query.is_empty() && request.get("filters").is_none() {
            return self.list(request);
        }
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
        let body = get_or_fixture(
            &format!("{BASE_URL}/browser?{query_string}"),
            LIST_FIXTURE,
            BASE_URL,
        );
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
        let details = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        let id = html::attr_after(&details, "data-id", "data-id")
            .or_else(|| html::attr(&details, "data-id"))
            .unwrap_or_else(|| path.rsplit('-').next().unwrap_or("1").to_string());
        let body = ajax_html(
            &format!(
                "{BASE_URL}/ajax/episodes/list?ani_id={id}&_={}",
                enc_endpoint(&id)
            ),
            EPISODES_FIXTURE,
            &absolute_url(&path),
        );
        let mut episodes = parse_episodes(&body, &request);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "sample-token".to_string());
        let id = episode
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("sample-token");
        let body = ajax_html(
            &format!(
                "{BASE_URL}/ajax/links/list?token={id}&_={}",
                enc_endpoint(id)
            ),
            HOSTERS_FIXTURE,
            &absolute_url(&episode),
        );
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
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/ajax/links/view?id={server_id}&_={}",
                enc_endpoint(server_id)
            ),
            r#"{"result":"https://megaup.cc/e/sample"}"#,
            referer,
        );
        let encoded = serde_json::from_str::<SourceLink>(&body)
            .ok()
            .and_then(|res| res.link.or(res.result))
            .unwrap_or_default();
        let link = decrypt_iframe(&encoded);
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
                title: "Updates".to_string(),
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn ajax_html(target: &str, fixture: &str, referer: &str) -> String {
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

fn enc_endpoint(input: &str) -> String {
    client(BASE_URL)
        .get(format!(
            "https://enc-dec.app/api/enc-kai?text={}",
            url::query_escape(input)
        ))
        .xhr()
        .referer(BASE_URL)
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<SourceLink>(&body).ok())
        .and_then(|res| res.result.or(res.link))
        .unwrap_or_else(|| input.to_string())
}

fn decrypt_iframe(input: &str) -> String {
    if input.starts_with("http") {
        return input.to_string();
    }
    let body = json!({ "text": input }).to_string();
    client(BASE_URL)
        .post("https://enc-dec.app/api/dec-kai")
        .xhr()
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header("Origin", BASE_URL)
        .referer(&format!("{BASE_URL}/watch"))
        .body(body.into_bytes())
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<IframeResponse>(&body).ok())
        .and_then(|res| res.result.url)
        .unwrap_or_else(|| input.to_string())
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_listing(body: &str, request: &Value) -> Paged<CatalogItem> {
    let chunks = if body.contains("aitem-wrapper") || body.contains(" aitem") {
        body.split("aitem")
            .skip(1)
            .filter(|chunk| chunk.contains("poster") || chunk.contains("title"))
            .collect::<Vec<_>>()
    } else {
        body.split("flw-item").skip(1).collect::<Vec<_>>()
    };
    Paged {
        entries: chunks
            .into_iter()
            .filter_map(|chunk| parse_card(chunk, request))
            .collect(),
        has_next_page: body.contains("pagination")
            && (body.contains("rel=\"next\"") || body.contains("active") && body.contains("<li")),
    }
}

fn parse_card(chunk: &str, request: &Value) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "poster", "href")
        .or_else(|| html::attr_after(chunk, "title", "href"))
        .or_else(|| html::attr_after(chunk, "film-poster-ahref", "href"))
        .or_else(|| html::attr_after(chunk, "dynamic-name", "href"))
        .or_else(|| html::attr_after(chunk, "film-detail", "href"))
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let path = path_key(&href);
    let use_english = preference(request, "preferred_title_lang")
        .map(|v| v == "English")
        .unwrap_or(false);
    let title = if use_english {
        html::attr_after(chunk, "a title", "title")
            .or_else(|| html::attr_after(chunk, "title", "title"))
            .or_else(|| html::attr_after(chunk, "dynamic-name", "title"))
            .or_else(|| html::attr_after(chunk, "film-detail", "title"))
            .or_else(|| html::attr_after(chunk, "<a", "title"))
    } else {
        html::attr_after(chunk, "title", "data-jp")
            .or_else(|| html::attr_after(chunk, "dynamic-name", "data-jp"))
            .or_else(|| html::attr_after(chunk, "dynamic-name", "data-jname"))
            .or_else(|| html::attr_after(chunk, "film-detail", "data-jname"))
            .or_else(|| html::attr_after(chunk, "<a", "data-jname"))
    }
    .filter(|value| !value.trim().is_empty())
    .or_else(|| {
        html::text_between(chunk, "title", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
    .unwrap_or_else(|| title_from_path(&path));
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: html::attr_after(chunk, "poster", "data-src")
            .or_else(|| html::attr_after(chunk, "poster", "src"))
            .or_else(|| html::attr_after(chunk, "film-poster", "data-src"))
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
    let info = body.split("main-entity").nth(1).unwrap_or(body);
    let detail = body.split("div detail").nth(1).unwrap_or(info);
    let title = html::text_between(body, "h1 title", "</h1>")
        .or_else(|| html::text_between(body, "title", "</h1>"))
        .or_else(|| html::text_between(body, "film-name", "</h2>"))
        .or_else(|| html::text_between(body, "film-name", "</h1>"))
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| title_from_path(path));
    let mut description = String::new();
    if let Some(overview) = html::text_between(info, "desc", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| info_value(info, "Overview:"))
    {
        description.push_str(&overview);
    }
    for tag in [
        "Country:",
        "Premiered:",
        "Date aired:",
        "Broadcast:",
        "Duration:",
        "MAL:",
    ] {
        if let Some(value) = info_value(detail, tag) {
            if !description.is_empty() {
                description.push('\n');
            }
            description.push_str(tag);
            description.push(' ');
            description.push_str(&value);
        }
    }
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: html::attr_after(body, "poster", "src")
            .or_else(|| html::attr_after(body, "poster", "data-src"))
            .or_else(|| html::attr_after(body, "anisc-poster", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: info_list(detail, "Genres:")
            .into_iter()
            .chain(info_list(info, "genre"))
            .collect(),
        authors: info_value(detail, "Studios:")
            .or_else(|| info_value(detail, "Producers:"))
            .into_iter()
            .collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&info_value(detail, "Status:").unwrap_or_default()),
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
    let value = html::strip_tags(block)
        .trim_start_matches(tag)
        .trim()
        .to_string();
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

fn parse_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let mark_fillers = preference_bool(request, "mark_fillers", true);
    let animekai = body.contains("token=");
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            if animekai && !chunk.contains("token=") {
                return None;
            }
            if !animekai && !chunk.contains("ep-item") {
                return None;
            }
            let href = html::attr(chunk, "href").unwrap_or_default();
            let token = html::attr(chunk, "token").unwrap_or_else(|| path_key(&href));
            let number = html::attr(chunk, "num")
                .or_else(|| html::attr(chunk, "data-number"))
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            let title = html::attr(chunk, "title")
                .or_else(|| {
                    html::text_between(chunk, "<span", "</span>")
                        .map(|text| html::strip_tags(&text))
                })
                .unwrap_or_else(|| title_from_path(&href));
            let filler = mark_fillers && chunk.contains("ssl-item-filler");
            Some(VideoEpisode {
                key: token,
                title: Some(if title.is_empty() {
                    format!("Episode {}", display_number(number))
                } else {
                    format!("Episode {}: {title}", display_number(number))
                }),
                episode_number: Some(number),
                url: (!href.is_empty()).then(|| absolute_url(&href)),
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
    for block in body.split("server-items").skip(1) {
        let type_id = html::attr(block, "data-id").unwrap_or_else(|| "softsub".to_string());
        if !enabled_types.iter().any(|value| value == &type_id) {
            continue;
        }
        let type_name = type_name(&type_id);
        for chunk in block.split("<span").skip(1) {
            let Some(server_id) = html::attr(chunk, "data-lid") else {
                continue;
            };
            let name = html::strip_tags(chunk.split("</span>").next().unwrap_or_default());
            if name.is_empty()
                || !enabled_hosters
                    .iter()
                    .any(|hoster| hoster.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            out.push(VideoHoster {
                key: format!("{server_id}|{type_name}|{name}|{episode_url}"),
                name: format!("{name} - {type_name}"),
                url: Some(episode_url.clone()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(&episode_url),
                ..VideoHoster::default()
            });
        }
    }
    if !out.is_empty() {
        return out;
    }
    for (type_id, type_name) in [
        ("sub", "[Hard Sub]"),
        ("softsub", "[Soft Sub]"),
        ("dub", "[Dub & S-Sub]"),
    ] {
        if !enabled_types.iter().any(|value| value == type_id) {
            continue;
        }
        let marker = format!("ps_-block-{type_id}");
        let raw_block = body.split(&marker).nth(1).unwrap_or_default();
        let block = raw_block.split("ps_-block-").next().unwrap_or(raw_block);
        for chunk in block.split("<a").skip(1) {
            let Some(server_id) = html::attr(chunk, "data-lid") else {
                continue;
            };
            let media_type = type_name.to_string();
            let name = html::strip_tags(chunk.split("</a>").next().unwrap_or_default());
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

fn type_name(type_id: &str) -> &'static str {
    match type_id {
        "sub" => "[Hard Sub]",
        "dub" => "[Dub & S-Sub]",
        "softsub" => "[Soft Sub]",
        _ => "[Soft Sub]",
    }
}

fn resolve_embed_streams(
    embed: &str,
    media_type: &str,
    name: &str,
    request: &Value,
) -> Vec<VideoStream> {
    if embed.is_empty() {
        return Vec::new();
    }
    if embed.contains(".m3u8") {
        return parse_hls(embed, media_type, name, embed, Vec::new(), request);
    }
    if let Some(streams) =
        resolve_megacloud(embed, media_type, name, request).filter(|streams| !streams.is_empty())
    {
        return streams;
    }
    let body = get_or_fixture(embed, "", BASE_URL);
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
    let Some(sources) = response.get("sources") else {
        return Some(Vec::new());
    };
    if encrypted && sources.is_string() {
        return Some(vec![external_stream(embed, media_type, name, request)]);
    }
    let mut out = Vec::new();
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
            let file = track.get("file")?.as_str()?;
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
}

fn is_preferred(quality: &str, server: &str, media_type: &str, request: &Value) -> bool {
    let pref_quality =
        preference(request, "preferred_quality").unwrap_or_else(|| "1080".to_string());
    let pref_server =
        preference(request, "preferred_server").unwrap_or_else(|| "Vidstreaming".to_string());
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
        .and_then(|p| p.get("hoster_selection"))
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
        .and_then(|p| p.get("type_selection"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            ["sub", "softsub", "dub"]
                .iter()
                .map(ToString::to_string)
                .collect()
        })
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
    (input.starts_with(BASE_URL) || input.starts_with("/watch/")).then(|| path_key(input))
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
        .unwrap_or("AnimeKai")
        .rsplit_once('-')
        .map(|(name, _)| name)
        .unwrap_or(input)
        .replace('-', " ")
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
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
    label
        .to_lowercase()
        .contains("english")
        .then(|| "en".to_string())
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
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

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
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
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    result: Option<String>,
}

#[derive(Deserialize)]
struct IframeResponse {
    result: IframeResult,
}

#[derive(Deserialize)]
struct IframeResult {
    url: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<div class="aitem-wrapper">
  <div class="aitem">
    <a class="poster" href="/watch/sample-anime"><img data-src="/poster.jpg"></a>
    <a class="title" href="/watch/sample-anime" title="Sample Anime" data-jp="Sample Anime">Sample Anime</a>
  </div>
</div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div id="main-entity" data-id="sample-anime-id">
  <h1 class="title" title="Sample Anime" data-jp="Sample Anime">Sample Anime</h1>
  <div class="poster"><img src="/poster.jpg"></div>
  <div class="desc">Sample overview.</div>
  <div class="detail">
    <div>Status: Releasing</div>
    <div>Studios: <a>Sample Studio</a></div>
    <div>Genres: <a>Action</a></div>
    <div>Country: Japan</div>
  </div>
</div>
"#;
const EPISODES_FIXTURE: &str = r#"<div class="eplist"><a token="sample-token" num="1" langs="2"><span>First Episode</span></a></div>"#;
const HOSTERS_FIXTURE: &str = r#"
<div class="server-items" data-id="softsub">
  <span class="server" data-lid="1">Server 1</span>
  <span class="server" data-lid="2">Server 2</span>
</div>
"#;

export_video_source!(SOURCE);
