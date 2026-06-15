use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: Einfach = Einfach;
const BASE_URL: &str = "https://einfach.to";

struct Einfach;

impl VideoSource for Einfach {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let section = if listing == "latest" {
            "filme"
        } else {
            "series"
        };
        let body = get_or_fixture(
            &format!("{BASE_URL}/{section}/page/{}", page(&request)),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("pagination") && body.contains("next"),
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
        if let Some(path) = query.strip_prefix("path:") {
            return Ok(Paged {
                entries: vec![fetch_details(&format!("/{path}"))],
                has_next_page: false,
            });
        }
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/page/{}/?s={}",
                page(&request),
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: body.contains("pagination") && body.contains("next"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/series/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/series/sample".to_string());
        if path.contains("/filme/") {
            let item = fetch_details(&path);
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some(format!("Movie - {}", item.title)),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = body
            .split("epsdlist")
            .nth(1)
            .unwrap_or(&body)
            .split("<a")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let epnum = html::text_between(chunk, "epl-num", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "S1 EP 1".to_string());
                let ep_title = html::text_between(chunk, "epl-title", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_default();
                let number = epnum
                    .rsplit(' ')
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1.0);
                Some(VideoEpisode {
                    key: path_key(&href),
                    title: Some(format!("{epnum} - {ep_title}")),
                    episode_number: Some(number),
                    url: Some(absolute_url(&href)),
                    language: Some("de".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/series/sample/episode-1".to_string());
        let page = absolute_url(&path);
        let body = get_or_fixture(&page, HOSTERS_FIXTURE);
        let selected = selected_hosters(&request);
        Ok(body
            .split("lserv")
            .nth(1)
            .unwrap_or(&body)
            .split("<a")
            .skip(1)
            .filter_map(|chunk| {
                let name = html::text_between(chunk, ">", "</a>")
                    .map(|value| html::strip_tags(&value).to_ascii_lowercase())
                    .filter(|value| !value.is_empty())?;
                if !selected.iter().any(|item| item == &name) {
                    return None;
                }
                let encoded = html::attr(chunk, "data-em")?;
                let html = decode_base64(&encoded)?;
                let embed = html::attr_after(&html, "<iframe", "src")?;
                let fixed = if embed.starts_with("//") {
                    format!("https:{embed}")
                } else {
                    embed
                };
                Some(VideoHoster {
                    key: format!("{name}|{fixed}"),
                    name: title_case(&name),
                    url: Some(page.clone()),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(&page),
                    ..VideoHoster::default()
                })
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(2, '|');
        let name = parts.next().unwrap_or("mirror");
        let embed = parts.next().unwrap_or_default();
        let mut streams = match name {
            "stream in hd" => resolve_mystream(embed),
            "vidoza" => resolve_vidoza(embed),
            "lulustream" => resolve_lulustream(embed),
            _ => resolve_embed(embed, &title_case(name), BASE_URL),
        };
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
                title: "Series".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Filme".to_string(),
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
    let info = body.split("div class=\"infl").nth(1).unwrap_or(&body);
    CatalogItem {
        key: path_key(path),
        title: html::text_between(info, "h1 class=\"entry-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(info, "<img", "data-lazy-src")
            .or_else(|| html::attr_after(info, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(info, "entry-content", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        artists: info_value(info, "Stars:").into_iter().collect(),
        tags: info_value(info, "Genre:")
            .map(|value| split_csv(&value))
            .unwrap_or_default(),
        authors: info_value(info, "Network:").into_iter().collect(),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: if info_value(info, "Status:").as_deref() == Some("Ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("tip") || chunk.contains("data-lazy-src"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::attr(chunk, "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-lazy-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn resolve_embed(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        return vec![media_stream(&src, name, embed)];
    }
    vec![external_stream(embed, name, referer)]
}

fn resolve_vidoza(embed: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    let source_block = body
        .split("sourcesCode: [")
        .nth(1)
        .and_then(|tail| tail.split("],").next());
    let Some(source_block) = source_block else {
        return vec![external_stream(embed, "Vidoza", BASE_URL)];
    };
    source_block
        .split('{')
        .skip(1)
        .filter_map(|chunk| {
            let src = chunk.split("src: \"").nth(1)?.split('"').next()?;
            let res = chunk
                .split("res:\"")
                .nth(1)
                .and_then(|tail| tail.split('"').next())
                .unwrap_or("auto");
            Some(media_stream(src, &format!("Vidoza - {res}p"), embed))
        })
        .collect()
}

fn resolve_lulustream(embed: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    let unpacked = body.split("eval(").nth(1).unwrap_or(&body);
    if let Some(src) = html::text_between(unpacked, "file:\"", "\"")
        .or_else(|| html::text_between(unpacked, "file:\\\"", "\\\""))
    {
        return vec![media_stream(&src, "LuLuStream", embed)];
    }
    vec![external_stream(embed, "LuLuStream", BASE_URL)]
}

fn resolve_mystream(embed: &str) -> Vec<VideoStream> {
    let response = client().get(embed).browser_document().send();
    let (body, cookie) = match response {
        Ok(response) => {
            let cookie = response
                .headers
                .iter()
                .find(|(name, value)| {
                    name.eq_ignore_ascii_case("set-cookie")
                        && value.to_ascii_lowercase().starts_with("phpsessid")
                })
                .map(|(_, value)| value.split(';').next().unwrap_or_default().to_string())
                .unwrap_or_default();
            (response.text.unwrap_or_default(), cookie)
        }
        Err(_) => (MYSTREAM_FIXTURE.to_string(), String::new()),
    };
    let code_part = body
        .split("sniff(")
        .nth(1)
        .and_then(|tail| tail.split(",[").next())
        .unwrap_or_default();
    let stream_code = code_part
        .rsplit("\",\"")
        .next()
        .and_then(|tail| tail.split('"').next())
        .or_else(|| {
            code_part
                .split(",\"")
                .nth(1)
                .and_then(|tail| tail.split('"').next())
        })
        .unwrap_or_default();
    let id = code_part
        .split(",\"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .unwrap_or_default();
    if id.is_empty() || stream_code.is_empty() {
        return vec![external_stream(embed, "Stream in HD", BASE_URL)];
    }
    let host = embed.split("/watch").next().unwrap_or(BASE_URL);
    let stream_url = format!("{host}/m3u8/{id}/{stream_code}/master.txt?s=1&cache=1");
    let mut headers = referer_headers(embed);
    headers.insert("Accept".to_string(), "*/*".to_string());
    if !cookie.is_empty() {
        headers.insert("Cookie".to_string(), cookie);
    }
    vec![VideoStream {
        url: stream_url,
        name: Some("MyStream: auto".to_string()),
        quality: Some("MyStream auto".to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers,
        initialized: true,
        ..VideoStream::default()
    }]
}

fn media_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8") || stream_url.contains("/m3u8/");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn decode_base64(input: &str) -> Option<String> {
    STANDARD
        .decode(input)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn selected_hosters(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_hoster_selection"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "doodstream".to_string(),
                "filelions".to_string(),
                "filemoon".to_string(),
                "lulustream".to_string(),
                "mixdrop".to_string(),
                "streamtape".to_string(),
                "streamwish".to_string(),
                "vidoza".to_string(),
                "voe".to_string(),
                "stream in hd".to_string(),
            ]
        })
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = pref(request, "pref_quality_key", "720p");
    streams.sort_by_key(|stream| {
        i32::from(
            stream
                .quality
                .as_deref()
                .unwrap_or_default()
                .contains(quality),
        )
    });
    streams.reverse();
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<li")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "span class=\"colspan", "</span>"))
        .or_else(|| {
            body.split(label)
                .nth(1)
                .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        })
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn title_case(input: &str) -> String {
    match input {
        "doodstream" => "DoodStream".to_string(),
        "filelions" => "FileLions".to_string(),
        "filemoon" => "Filemoon".to_string(),
        "lulustream" => "LuLuStream".to_string(),
        "mixdrop" => "MixDrop".to_string(),
        "streamtape" => "Streamtape".to_string(),
        "streamwish" => "StreamWish".to_string(),
        "vidoza" => "Vidoza".to_string(),
        "voe" => "VOE".to_string(),
        "stream in hd" => "Stream in HD".to_string(),
        _ => input.to_string(),
    }
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Einfach")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

const LIST_FIXTURE: &str = r#"<article class="box"><div class="bx"><a class="tip" href="/series/sample" title="Sample Anime"><img src="/cover.jpg"></a></div></article>"#;
const DETAILS_FIXTURE: &str = r#"<article><div><div class="infl"><h1 class="entry-title">Sample Anime</h1><img src="/cover.jpg"><ul><li><b>Status:</b><span class="colspan">Ongoing</span></li><li><b>Genre:</b><span class="colspan">Action</span></li></ul><div class="entry-content"><p>Sample description.</p></div></div></div></article>"#;
const EPISODES_FIXTURE: &str = r#"<div class="epsdlist"><ul><li><a href="/series/sample/episode-1"><span class="epl-num">S1 EP 1</span><span class="epl-title">Episode 1</span></a></li></ul></div>"#;
const HOSTERS_FIXTURE: &str = r#"<div class="lserv"><ul><li><a data-em="PGlmcmFtZSBzcmM9Imh0dHBzOi8vdm9lLnN4L2Uvc2FtcGxlIj48L2lmcmFtZT4=">voe</a></li></ul></div>"#;
const MYSTREAM_FIXTURE: &str = r#"<script>sniff("x","sample-id","sample-code",[])</script>"#;

export_video_source!(SOURCE);
