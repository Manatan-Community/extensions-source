use base64::{Engine, engine::general_purpose::STANDARD};
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

const SOURCE: Tuktukcinema = Tuktukcinema;
const DEFAULT_BASE_URL: &str = "https://tuktukhd.com";

struct Tuktukcinema;

impl VideoSource for Tuktukcinema {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let page = page(&request);
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{base_url}/recent/page/{page}/")
        } else {
            format!("{base_url}/main/")
        };
        let body = get_or_fixture(&base_url, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base_url),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query, &base_url) {
            return Ok(Paged {
                entries: vec![fetch_details(&base_url, &path)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{base_url}/?s={}&page={page}",
                manatan_shared::sdk::http::url_encode(query)
            )
        } else if let Some(filter_url) = filter_url(&request, &base_url, page) {
            filter_url
        } else {
            format!("{base_url}/main/")
        };
        let body = get_or_fixture(&base_url, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body, &base_url),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base_url = base_url(&request);
        let path =
            request_key(&request, "item", &base_url).unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&base_url, &path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base_url = base_url(&request);
        let path =
            request_key(&request, "item", &base_url).unwrap_or_else(|| "/sample".to_string());
        let body = get_or_fixture(&base_url, &absolute_url(&base_url, &path), EPISODES_FIXTURE);
        let seasons = parse_season_links(&body, &base_url);
        if seasons.is_empty() {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("مشاهدة".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&base_url, &path)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let selected_season = html::text_between(&body, "mpbreadcrumbs", "</div>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (season_index, (season_name, season_path)) in seasons.iter().rev().enumerate() {
            let season_body = if selected_season.contains(season_name) {
                body.clone()
            } else {
                get_or_fixture(
                    &base_url,
                    &absolute_url(&base_url, season_path),
                    EPISODES_FIXTURE,
                )
            };
            let season_num = if seasons.len() == 1 {
                1
            } else {
                first_number(season_name).unwrap_or(season_index as f32 + 1.0) as i32
            };
            out.extend(parse_episode_links(
                &season_body,
                &base_url,
                season_name,
                season_num,
            ));
        }
        Ok(out)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base_url = base_url(&request);
        let path =
            request_key(&request, "episode", &base_url).unwrap_or_else(|| "/sample".to_string());
        let body = get_or_fixture(&base_url, &absolute_url(&base_url, &path), HOSTERS_FIXTURE);
        Ok(parse_hosters(&body, &base_url))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base_url = base_url(&request);
        let key = request_key(&request, "hoster", &base_url).unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed_streams(&base_url, &key, name);
        sort_streams(&mut streams, &preferred_quality(&request));
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
        sort_streams(&mut streams, &preferred_quality(&request));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Main".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recent".to_string(),
                entries: self.list(json!({"listing": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?.entries,
                has_more: true,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url(&request);
        Ok(request_key(&request, "item", &base_url).map(|path| absolute_url(&base_url, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url(&request);
        Ok(request_key(&request, "episode", &base_url).map(|path| absolute_url(&base_url, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base_url = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input, &base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&base_url, &path)),
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

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{base_url}/"))
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(base_url: &str, target: &str, fixture: &str) -> String {
    client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(base_url: &str, path: &str) -> CatalogItem {
    let body = get_or_fixture(base_url, &absolute_url(base_url, path), DETAILS_FIXTURE);
    CatalogItem {
        key: path_key(path, base_url),
        title: html::text_between(&body, "post-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| edit_title(&html::strip_tags(&value), false))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "div class=\"left", "data-src")
            .or_else(|| html::attr_after(&body, "div class=\"left", "src"))
            .or_else(|| html::attr_after(&body, "<img", "data-src"))
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(base_url, image.split('?').next().unwrap_or(&image))),
        url: Some(absolute_url(base_url, path)),
        description: html::text_between(&body, "div class=\"story", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: collect_anchor_text(&body, "catssection"),
        authors: collect_anchor_text(&body, "RightTaxContent"),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str, base_url: &str) -> Vec<CatalogItem> {
    let normalized = body.replace("Small--Box", "Block--Item");
    normalized
        .split("Block--Item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value))
                })
                .map(|value| edit_title(&value, true))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href, base_url),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
                    .or_else(|| {
                        html::attr_after(chunk, "<img", "srcset").map(|value| {
                            value
                                .split_whitespace()
                                .next()
                                .unwrap_or(&value)
                                .to_string()
                        })
                    })
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(base_url, image.split('?').next().unwrap_or(&image))),
                url: Some(absolute_url(base_url, &href)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Completed,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_season_links(body: &str, base_url: &str) -> Vec<(String, String)> {
    body.split("allseasonss")
        .nth(1)
        .unwrap_or_default()
        .split("Block--Item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let name = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Season".to_string());
            Some((name, path_key(&href, base_url)))
        })
        .collect()
}

fn parse_episode_links(
    body: &str,
    base_url: &str,
    season_name: &str,
    season_num: i32,
) -> Vec<VideoEpisode> {
    body.split("allepcont")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let episode_num = html::text_between(chunk, "epnum", "</div>")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| first_number(&value))
                .unwrap_or(1.0);
            Some(VideoEpisode {
                key: path_key(&href, base_url),
                title: Some(format!("{season_name} : الحلقة {episode_num:.0}")),
                episode_number: format!("{season_num}.{episode_num:03.0}").parse().ok(),
                season_number: Some(season_num as f32),
                url: Some(absolute_url(base_url, &href)),
                language: Some("ar".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, base_url: &str) -> Vec<VideoHoster> {
    body.split("server--item")
        .skip(1)
        .filter_map(|chunk| {
            let encoded = html::attr(chunk, "data-link")?
                .substring_before("0REL0Y")
                .chars()
                .rev()
                .collect::<String>();
            let decoded = STANDARD
                .decode(encoded)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())?;
            let name = html::strip_tags(chunk).trim().to_string();
            Some(VideoHoster {
                key: decoded.clone(),
                name: if name.is_empty() {
                    hoster_name(&decoded)
                } else {
                    name
                },
                url: Some(decoded),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(base_url),
                ..VideoHoster::default()
            })
        })
        .collect()
}

trait SubstringBefore {
    fn substring_before(&self, marker: &str) -> &str;
}

impl SubstringBefore for str {
    fn substring_before(&self, marker: &str) -> &str {
        self.split(marker).next().unwrap_or(self)
    }
}

fn resolve_embed_streams(base_url: &str, embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains("iframe") {
        let body = get_or_fixture(base_url, embed, "");
        let mut out = Vec::new();
        for chunk in body.split("<iframe").skip(1) {
            if let Some(src) = html::attr(chunk, "src") {
                out.extend(resolve_embed_streams(
                    base_url,
                    &absolute_url(base_url, &src),
                    &hoster_name(&src),
                ));
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if embed.contains(".m3u8") {
        return parse_hls(base_url, embed, name, embed);
    }
    if embed.contains("krakenfiles") {
        let body = get_or_fixture(base_url, embed, "");
        if let Some(src) = html::attr_after(&body, "<source", "src") {
            return vec![media_stream(&src, name, "direct", embed)];
        }
    }
    let body = get_or_fixture(base_url, embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        if src.contains(".m3u8") {
            return parse_hls(base_url, &src, name, embed);
        }
        return vec![media_stream(&src, name, "direct", embed)];
    }
    vec![external_stream(embed, name, base_url)]
}

fn parse_hls(base_url: &str, target: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client(base_url).get(target).send_text().unwrap_or_default();
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
            let stream_url = if line.starts_with("http") {
                line.to_string()
            } else {
                format!(
                    "{}/{}",
                    target
                        .rsplit_once('/')
                        .map(|(base, _)| base)
                        .unwrap_or(target),
                    line
                )
            };
            Some(media_stream(&stream_url, name, &quality, referer))
        })
        .collect()
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.to_string()),
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
        quality: Some(hoster_name(stream_url)),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn filter_url(request: &Value, base_url: &str, page: u64) -> Option<String> {
    let section = filter(request, "section");
    let genre = filter(request, "genre");
    let advanced_section = filter(request, "advanced_section");
    let advanced_genre = filter(request, "advanced_genre");
    let advanced_rating = filter(request, "advanced_rating");
    if !advanced_section.is_empty() || !advanced_genre.is_empty() || !advanced_rating.is_empty() {
        let mut out = format!("{base_url}/filtering/?pagenum={page}");
        if !advanced_section.is_empty() {
            out.push_str("&category=");
            out.push_str(&advanced_section);
        }
        if !advanced_genre.is_empty() {
            out.push_str("&genre=");
            out.push_str(&advanced_genre);
        }
        if !advanced_rating.is_empty() {
            out.push_str("&mpaa=");
            out.push_str(&advanced_rating);
        }
        Some(out)
    } else if !section.is_empty() {
        Some(format!(
            "{base_url}/{}?page={page}",
            section.trim_start_matches('/')
        ))
    } else if !genre.is_empty() {
        Some(format!("{base_url}/genre/{genre}/?page={page}"))
    } else {
        None
    }
}

fn edit_title(title: &str, details: bool) -> String {
    let title = title.trim();
    for marker in ["فيلم ", "عرض "] {
        if let Some(rest) = title.strip_prefix(marker) {
            if let Some((name, suffix)) = rest.rsplit_once(' ') {
                return if details {
                    format!("{name} ({suffix})")
                } else {
                    name.to_string()
                };
            }
        }
    }
    for marker in ["مسلسل ", "برنامج ", "انمي "] {
        if let Some(rest) = title.strip_prefix(marker) {
            if let Some((name, ep)) = rest.rsplit_once(" الحلقة ") {
                return if details {
                    format!("{name} (ep:{ep})")
                } else if name.contains("الموسم") {
                    name.split("الموسم")
                        .next()
                        .unwrap_or(name)
                        .trim()
                        .to_string()
                } else {
                    name.trim().to_string()
                };
            }
        }
    }
    title.to_string()
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

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("custom_domain"))
        .or_else(|| request.get("custom_domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http") && !value.ends_with('/'))
        .unwrap_or(DEFAULT_BASE_URL)
        .to_string()
}

fn filter(request: &Value, key: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080")
        .to_string()
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let digits = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (i32::from(quality.contains(preferred)), digits)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred))
            .unwrap_or(false);
    }
}

fn hoster_name(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if lower.contains("mixdrop") {
        "Mixdrop".to_string()
    } else if lower.contains("dood") {
        "Dood".to_string()
    } else if lower.contains("lulustream") || lower.contains("streamwish") {
        "StreamWish".to_string()
    } else if lower.contains("krakenfiles") {
        "Kraken".to_string()
    } else if lower.contains("earnvids") {
        "Earnvids".to_string()
    } else if lower.contains("vidbom") || lower.contains("vidshare") || lower.contains("govid") {
        "VidBom".to_string()
    } else {
        input
            .split("://")
            .nth(1)
            .unwrap_or(input)
            .split('/')
            .next()
            .unwrap_or("Mirror")
            .replace("www.", "")
    }
}

fn first_number(input: &str) -> Option<f32> {
    input
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>()
        .parse()
        .ok()
}

fn request_key(request: &Value, field: &str, base_url: &str) -> Option<String> {
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
        .map(|value| path_key(value, base_url))
}

fn path_from_url(input: &str, base_url: &str) -> Option<String> {
    if input.starts_with(base_url) || input.starts_with(DEFAULT_BASE_URL) {
        Some(path_key(input, base_url))
    } else {
        None
    }
}

fn path_key(input: &str, base_url: &str) -> String {
    if input.starts_with("http")
        && !input.starts_with(base_url)
        && !input.starts_with(DEFAULT_BASE_URL)
    {
        return input.to_string();
    }
    let path = input
        .strip_prefix(base_url)
        .or_else(|| input.strip_prefix(DEFAULT_BASE_URL))
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(base_url: &str, input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(base_url, input)
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("TukTukCinema")
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

fn has_next_page(body: &str) -> bool {
    body.contains("page-numbers") && body.contains("next")
}

const LIST_FIXTURE: &str = r#"<div class="Block--Item"><a href="/movie/sample" title="فيلم Sample 2024 مترجم"><img src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="post-title">فيلم Sample 2024 مترجم</h1><div class="left"><div class="image"><img src="/cover.jpg"></div></div><div class="story">Sample description.</div><div class="catssection"><li><a>اكشن</a></li></div>"#;
const EPISODES_FIXTURE: &str = r#"<section class="allseasonss"><div class="Block--Item"><a href="/series/sample-season-1"><h3>الموسم 1</h3></a></div></section><section class="allepcont"><a href="/episode/sample-1"><div class="epnum">1</div></a></section>"#;
const HOSTERS_FIXTURE: &str = r#"<li class="server--item" data-link="=ATbhR2cvxWY2NXYlxWYoR3clN2c092YzJXZk9SZ0lGdzVmX0VWZ0JCL">voe</li>"#;

export_video_source!(SOURCE);
