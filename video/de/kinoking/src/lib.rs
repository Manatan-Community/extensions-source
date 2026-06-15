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

const SOURCE: Kinoking = Kinoking;
const BASE_URL: &str = "https://kinoking.cc";

struct Kinoking;

impl VideoSource for Kinoking {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/episodes/page/{page}")
        } else if page > 1 {
            format!("{BASE_URL}/page/{page}")
        } else {
            BASE_URL.to_string()
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        let entries = if listing == "latest" {
            parse_cards(&body, "div.content article > div.poster")
        } else {
            parse_cards(&body, "div#featured-titles div.poster")
        };
        Ok(Paged {
            entries,
            has_next_page: body.contains("fa-chevron-right") || body.contains("nextpagination"),
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
        let genre = filter(&request, "genre");
        let target = if query.is_empty() && !genre.is_empty() {
            format!("{BASE_URL}/{genre}/page/{page}")
        } else {
            format!(
                "{BASE_URL}/page/{page}/?s={}",
                manatan_shared::sdk::http::url_encode(query)
            )
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, "div.result-item div.image a"),
            has_next_page: body.contains("fa-chevron-right") || body.contains("resppages"),
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
        let real_body = if let Some(details_url) = html::attr_after(&body, "fa-bars", "href") {
            get_or_fixture(&absolute_url(&details_url), &body)
        } else {
            body
        };
        let mut episodes = parse_episodes(&real_body);
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: path_key(&page_url),
                title: Some("Film".to_string()),
                episode_number: Some(1.0),
                url: Some(page_url),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            });
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let page_url = absolute_url(&path);
        let body = get_or_fixture(&page_url, HOSTERS_FIXTURE);
        let selected = selected_hosters(&request);
        Ok(parse_hoster_options(&body, &page_url)
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
        let mut streams = resolve_player(&key, name);
        sort_streams(&mut streams, pref(&request, "preferred_hoster", "https://dood"));
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
        sort_streams(&mut streams, pref(&request, "preferred_hoster", "https://dood"));
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Featured".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular"}))?.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Episodes".to_string(),
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
    let header = body.split("div class=\"sheader").nth(1).unwrap_or(&body);
    CatalogItem {
        key: path_key(path),
        title: html::attr_after(header, "div class=\"poster", "alt")
            .or_else(|| html::text_between(header, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(header, "div class=\"poster", "data-src")
            .or_else(|| html::attr_after(header, "div class=\"poster", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(&body, "div id=\"info", "</p>").map(|value| html::strip_tags(&value)),
        tags: collect_anchor_text(header, "sgeneros"),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str, marker: &str) -> Vec<CatalogItem> {
    let chunk = body.split(marker).nth(1).unwrap_or(body);
    chunk
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            let href = html::attr(part, "href")?;
            let title = html::attr_after(part, "<img", "alt").unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(part, "<img", "data-src")
                    .or_else(|| html::attr_after(part, "<img", "data-lazy-src"))
                    .or_else(|| html::attr_after(part, "<img", "src"))
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

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let mut out = Vec::new();
    for season in body.split("div id=\"seasons").skip(1).flat_map(|chunk| chunk.split("<div").skip(1)) {
        if !season.contains("se-t") {
            continue;
        }
        let season_name = html::text_between(season, "span class=\"se-t", "</span>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| "1".to_string());
        for ep in season.split("ul class=\"episodios").skip(1).flat_map(|chunk| chunk.split("<li").skip(1)) {
            let Some(href) = html::attr_after(ep, "<a", "href") else {
                continue;
            };
            let ep_num = html::text_between(ep, "div class=\"numerando", "</div>")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| value.rsplit(|ch: char| !ch.is_ascii_digit()).find(|v| !v.is_empty()).and_then(|v| v.parse::<f32>().ok()))
                .unwrap_or(0.0);
            let ep_name = html::text_between(ep, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Folge {ep_num}"));
            out.push(VideoEpisode {
                key: path_key(&href),
                title: Some(format!("Staffel {season_name} Folge {ep_num} : {ep_name}")),
                episode_number: Some(ep_num),
                season_number: season_name.parse().ok(),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out
}

fn parse_hoster_options(body: &str, page_url: &str) -> Vec<VideoHoster> {
    body.split("li class=\"dooplay_player_option")
        .skip(1)
        .filter_map(|chunk| {
            let post = html::attr(chunk, "data-post")?;
            let nume = html::attr(chunk, "data-nume")?;
            let kind = html::attr(chunk, "data-type")?;
            let name = html::text_between(chunk, "span class=\"title", "</span>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Mirror".to_string());
            let key = format!("{page_url}|{post}|{nume}|{kind}|{}", classify_hoster(&name));
            Some(VideoHoster {
                key,
                name,
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(page_url),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_player(key: &str, name: &str) -> Vec<VideoStream> {
    let parts = key.split('|').collect::<Vec<_>>();
    if parts.len() < 4 {
        return vec![external_stream(key, name, BASE_URL)];
    }
    let page_url = parts[0];
    let body = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(&[
            ("action", "doo_player_ajax"),
            ("post", parts[1]),
            ("nume", parts[2]),
            ("type", parts[3]),
        ])
        .referer(page_url)
        .xhr()
        .send_text()
        .unwrap_or_default();
    let embed = html::text_between(&body, "\"embed_url\":\"", "\",")
        .map(|value| value.replace("\\/", "/").replace("\\", ""))
        .unwrap_or_else(|| key.to_string());
    resolve_embed_streams(&normalize_scheme(&embed), name, page_url)
}

fn resolve_embed_streams(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client().get(embed).referer(referer).send_text().unwrap_or_default();
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        return vec![media_stream(&absolute_remote(&src, embed), name, "direct", embed)];
    }
    vec![external_stream(embed, name, referer)]
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

fn selected_hosters(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("hoster_selection"))
        .and_then(Value::as_array)
    else {
        return vec!["dood".to_string(), "voe".to_string(), "filehosted".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn classify_hoster(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("dood") {
        "dood"
    } else if lower.contains("voe") {
        "voe"
    } else if lower.contains("filehosted") {
        "filehosted"
    } else {
        "other"
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred_hoster: &str) {
    streams.sort_by_key(|stream| i32::from(stream.url.contains(preferred_hoster)));
    streams.reverse();
    for stream in streams {
        stream.preferred = stream.url.contains(preferred_hoster);
    }
}

fn filter(request: &Value, key: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_matches('/')
        .to_string()
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
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
        .unwrap_or("Kinoking")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

const LIST_FIXTURE: &str = r#"
<div id="featured-titles"><div class="poster"><a href="/movies/sample"><img alt="Sample" src="/poster.jpg"></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="sheader"><div class="poster"><img alt="Sample" src="/poster.jpg"></div><div class="data"><h1>Sample</h1><div class="sgeneros"><a>Action</a></div></div></div>
<div id="info"><p>Beschreibung</p></div>
"#;

const HOSTERS_FIXTURE: &str = r#"
<li class="dooplay_player_option" data-post="1" data-nume="1" data-type="movie"><span class="title">Voe</span></li>
"#;

export_video_source!(SOURCE);
