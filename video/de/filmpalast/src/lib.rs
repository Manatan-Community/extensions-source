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

const SOURCE: FilmPalast = FilmPalast;
const BASE_URL: &str = "https://filmpalast.to";

struct FilmPalast;

impl VideoSource for FilmPalast {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/page/{page}")
        } else {
            format!("{BASE_URL}/movies/top/page/{page}")
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
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/search/title/{}/{page}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let page_url = absolute_url(&path);
        let body = get_or_fixture(&page_url, DETAILS_FIXTURE);
        let canonical = html::attr_after(&body, "rel=\"canonical\"", "href").unwrap_or(page_url);
        Ok(vec![VideoEpisode {
            key: path_key(&canonical),
            title: Some("Film".to_string()),
            episode_number: Some(1.0),
            url: Some(canonical),
            language: Some("de".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let page_url = absolute_url(&path);
        let body = get_or_fixture(&page_url, HOSTERS_FIXTURE);
        let selected = selected_hosters(&request);
        Ok(parse_hosters(&body, &page_url)
            .into_iter()
            .filter(|hoster| selected.iter().any(|key| hoster.key.contains(key)))
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed_streams(&key, name, BASE_URL);
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://voe.sx"),
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
            pref(&request, "preferred_hoster", "https://voe.sx"),
        );
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Top Filme".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular"}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Neue Filme".to_string(),
                entries: self.list(json!({"listing": "latest"}))?.entries,
                has_more: true,
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
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "h2 class=\"bgDark", "</h2>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "img class=\"cover2", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: detail_text(&body, 3),
        tags: detail_text(&body, 2)
            .map(|text| split_csv(&text))
            .unwrap_or_default(),
        authors: detail_text(&body, 4)
            .map(|text| split_csv(&text))
            .unwrap_or_default(),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("liste"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Completed,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, page_url: &str) -> Vec<VideoHoster> {
    body.split("ul class=\"currentStreamLinks")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")
                .or_else(|| html::attr(chunk, "data-player-url"))
                .map(|value| absolute_url(&value))?;
            let name = hoster_name(&href);
            Some(VideoHoster {
                key: href.clone(),
                name,
                url: Some(href),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(page_url),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_embed_streams(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let embed = normalize_scheme(embed);
    if embed.contains("streamtape") {
        if let Some(stream) = resolve_streamtape(&embed, name, referer) {
            return vec![stream];
        }
    }
    if embed.contains("evoload") {
        if let Some(stream) = resolve_evoload(&embed, name, referer) {
            return vec![stream];
        }
    }
    let body = get_or_fixture(&embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
        .or_else(|| html::text_between(&body, "{file:\"", "\""))
        .or_else(|| html::text_between(&body, "{file:\\\"", "\\\""))
    {
        return streams_from_media_url(&absolute_remote(&src, &embed), name, &embed);
    }
    vec![external_stream(&embed, name, referer)]
}

fn resolve_streamtape(embed: &str, name: &str, referer: &str) -> Option<VideoStream> {
    let body = client()
        .get(embed)
        .header(
            "Cookie",
            "Fuck Streamtape because they add concatenation to fuck up scrapers",
        )
        .referer(referer)
        .send_text()
        .ok()?;
    let script = body
        .split("document.getElementById('robotlink')")
        .nth(1)?
        .split("innerHTML = '")
        .nth(1)?;
    let first = script.split('\'').next()?;
    let second = script
        .split("+ ('xcd")
        .nth(1)
        .and_then(|part| part.split('\'').next())
        .unwrap_or_default();
    Some(media_stream(
        &format!("https:{first}{second}"),
        name,
        "Streamtape",
        embed,
    ))
}

fn resolve_evoload(embed: &str, name: &str, referer: &str) -> Option<VideoStream> {
    let id = embed.rsplit('/').next()?.trim();
    let csrv = HttpClient::browser()
        .get("https://csrv.evosrv.com/captcha?m412548=")
        .send_text()
        .ok()?;
    let pass_body = HttpClient::browser()
        .get("https://cd2.evosrv.com/html/jsx/e.jsx")
        .send_text()
        .unwrap_or_default();
    let pass = html::text_between(&pass_body, "var captcha_pass = '", "'").unwrap_or_default();
    let body = HttpClient::browser()
        .post("https://evoload.io/SecurePlayer")
        .json(format!(
            r#"{{"code":"{id}","token":"ok","csrv_token":"{csrv}","pass":"{pass}","reff":"{referer}/"}}"#
        ))
        .send_text()
        .ok()?;
    if body.contains("\"xstatus\":\"del") {
        return None;
    }
    let src = html::text_between(&body, "\"encoded_src\":\"", "\",")
        .or_else(|| html::text_between(&body, "\"src\":\"", "\","))?;
    Some(media_stream(&src.replace("\\/", "/"), name, "Evoload", embed))
}

fn streams_from_media_url(url: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    if url.contains(".m3u8") {
        return parse_hls(url, name, referer);
    }
    vec![media_stream(url, name, "direct", referer)]
}

fn parse_hls(target: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client().get(target).referer(referer).send_text().unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(target, name, "auto", referer)];
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
            let stream_url = absolute_remote(line, target);
            Some(media_stream(&stream_url, name, &quality, referer))
        })
        .collect()
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(format!("{name} {quality}")),
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

fn detail_text(body: &str, child: usize) -> Option<String> {
    body.split(&format!("li:nth-child({child})"))
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
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

fn selected_hosters(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("hoster_selection"))
        .and_then(Value::as_array)
    else {
        return vec![
            "voe".to_string(),
            "streamtape".to_string(),
            "evoload".to_string(),
            "wolfstream".to_string(),
        ];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| match value {
            "stape" => "streamtape",
            "evo" => "evoload",
            "wolf" => "wolfstream",
            value => value,
        })
        .map(ToString::to_string)
        .collect()
}

fn sort_streams(streams: &mut [VideoStream], preferred_hoster: &str) {
    streams.sort_by_key(|stream| {
        i32::from(
            stream.url.contains(preferred_hoster)
                || stream
                    .quality
                    .as_deref()
                    .map(|quality| quality.contains(preferred_hoster))
                    .unwrap_or(false),
        )
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream.url.contains(preferred_hoster)
            || stream
                .quality
                .as_deref()
                .map(|quality| quality.contains(preferred_hoster))
                .unwrap_or(false);
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
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn absolute_remote(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(base, input)
    }
}

fn normalize_scheme(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_string()
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("FilmPalast")
        .replace('-', " ")
}

fn hoster_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("streamtape") {
        "Streamtape"
    } else if lower.contains("evoload") {
        "Evoload"
    } else if lower.contains("wolfstream") {
        "WolfStream"
    } else if lower.contains("voe") {
        "Voe"
    } else {
        "Mirror"
    }
    .to_string()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("pageing") && body.contains("vorw")
}

const LIST_FIXTURE: &str = r#"
<article class="liste"><a href="/movie/sample" title="Sample Film"><img src="/img/sample.jpg"></a></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<link rel="canonical" href="https://filmpalast.to/movie/sample">
<h2 class="bgDark">Sample Film</h2><img class="cover2" src="/img/sample.jpg">
<ul id="detail-content-list"><li></li><li><span>Action, Drama</span></li><li><span>Beschreibung</span></li><li><span>Regie</span></li></ul>
"#;

const HOSTERS_FIXTURE: &str = r#"
<ul class="currentStreamLinks"><li><a href="https://voe.sx/e/sample">Voe</a></li></ul>
"#;

export_video_source!(SOURCE);
