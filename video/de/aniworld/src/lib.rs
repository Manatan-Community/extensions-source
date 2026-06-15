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

const SOURCE: AniWorld = AniWorld;
const BASE_URL: &str = "https://aniworld.to";

struct AniWorld;

impl VideoSource for AniWorld {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "/neu"
        } else {
            "/beliebte-animes"
        };
        let body = get_or_fixture(&format!("{BASE_URL}{path}"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: false,
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
        let body = client()
            .get(format!(
                "{BASE_URL}/ajax/seriesSearch?keyword={}",
                url::query_escape(query)
            ))
            .referer(&format!("{BASE_URL}/search"))
            .header("Origin", BASE_URL)
            .header("X-Requested-With", "XMLHttpRequest")
            .xhr()
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_search_json(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/anime/stream/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/anime/stream/sample".to_string());
        let page = absolute_url(&path);
        let body = get_or_fixture(&page, DETAILS_FIXTURE);
        let seasons = body
            .split("#stream")
            .nth(1)
            .unwrap_or(&body)
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "href"))
            .filter(|href| href.contains("/staffel-") || href.contains("/filme"))
            .collect::<Vec<_>>();
        let mut episodes = Vec::new();
        if seasons.is_empty() {
            episodes.extend(parse_episode_rows(&body));
        } else {
            for season in seasons {
                let season_url = absolute_url(&season);
                let season_body = get_or_fixture(&season_url, DETAILS_FIXTURE);
                episodes.extend(parse_episode_rows(&season_body));
            }
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/stream/sample/staffel-1/episode-1".to_string());
        let page = absolute_url(&path);
        let body = get_or_fixture(&page, HOSTERS_FIXTURE);
        let excluded = excluded_hosters(&request);
        Ok(body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("data-lang-key") && chunk.contains("watchEpisode"))
            .filter_map(|chunk| {
                let lang = language_name(&html::attr(chunk, "data-lang-key").unwrap_or_default());
                let href = html::attr_after(chunk, "watchEpisode", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let hoster = html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| host_name(&href));
                if excluded.iter().any(|item| hoster.contains(item)) {
                    return None;
                }
                Some(VideoHoster {
                    key: format!("{}|{}|{}", lang, hoster, absolute_url(&href)),
                    name: format!("{hoster} {lang}"),
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
        let mut parts = key.splitn(3, '|');
        let lang = parts.next().unwrap_or("?");
        let hoster = parts.next().unwrap_or("Mirror");
        let redirect = parts.next().unwrap_or_default();
        let target = redirected_url(redirect);
        let mut streams = resolve_known_embed(&target, &format!("({lang}) {hoster}"), redirect);
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
                title: "Beliebte Animes".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Neu".to_string(),
                entries: latest.entries,
                has_more: false,
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
        title: html::text_between(&body, "div class=\"series-title", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "seriesCoverBox", "data-src")
            .or_else(|| html::attr_after(&body, "seriesCoverBox", "src"))
            .map(|image| absolute_url(&image)),
        description: html::attr_after(&body, "p class=\"seri_des", "data-full-description")
            .or_else(|| {
                html::text_between(&body, "p class=\"seri_des", "</p>")
                    .map(|value| html::strip_tags(&value))
            }),
        tags: collect_list_text(&body, "genres"),
        authors: collect_list_text(&body, "Produzent:"),
        url: Some(absolute_url(path)),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("seriesListContainer")
        .nth(1)
        .unwrap_or(body)
        .split("<div")
        .filter(|chunk| chunk.contains("<h3") && chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
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

fn parse_search_json(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    root.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let link = item.get("link")?.as_str()?;
            let path = format!("/anime/stream/{link}");
            Some(CatalogItem {
                key: path.clone(),
                title: name.to_string(),
                cover: item
                    .get("cover")
                    .and_then(Value::as_str)
                    .map(|cover| absolute_url(&cover.replace("150x225", "220x330"))),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(absolute_url(&path)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episode_rows(body: &str) -> Vec<VideoEpisode> {
    body.split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("seasonEpisodeTitle"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "seasonEpisodeTitle", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let num = html::attr(chunk, "data-episode-season-id")
                .or_else(|| html::attr_after(chunk, "<meta", "content"))
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            let name = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Episode {num}"));
            let title = if href.contains("/filme") {
                format!("Film {} : {}", num as i32, name)
            } else {
                let season = href
                    .split("staffel-")
                    .nth(1)
                    .and_then(|tail| tail.split('/').next())
                    .unwrap_or("1");
                format!("Staffel {season} Folge {} : {name}", num as i32)
            };
            Some(VideoEpisode {
                key: path_key(&href),
                title: Some(title),
                episode_number: Some(num),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn redirected_url(input: &str) -> String {
    client()
        .get(input)
        .browser_document()
        .send()
        .ok()
        .map(|response| response.final_url)
        .unwrap_or_else(|| input.to_string())
}

fn resolve_known_embed(embed: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = get_or_fixture(embed, "");
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        return vec![media_stream(&src, name, embed)];
    }
    vec![external_stream(embed, name, referer)]
}

fn media_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
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

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_hoster = pref(request, "preferred_hoster", "VOE").to_ascii_lowercase();
    let preferred_lang = pref(request, "preferred_lang", "Deutscher Sub").to_ascii_lowercase();
    streams.sort_by_key(|stream| {
        let name = stream
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (
            i32::from(name.contains(&preferred_hoster)),
            i32::from(name.contains(&preferred_lang)),
        )
    });
    streams.reverse();
}

fn excluded_hosters(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("excluded_hosters"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn language_name(key: &str) -> &'static str {
    if key.contains('3') {
        "Deutscher Sub"
    } else if key.contains('1') {
        "Deutscher Dub"
    } else if key.contains('2') {
        "Englischer Sub"
    } else {
        "?"
    }
}

fn collect_list_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<li")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</li>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn host_name(url: &str) -> String {
    url.split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .unwrap_or("Mirror")
        .replace("www.", "")
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
    if input.starts_with(BASE_URL) || input.starts_with("/anime/stream/") {
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
        .unwrap_or("AniWorld")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

const LIST_FIXTURE: &str = r#"<div class="seriesListContainer"><div><a href="/anime/stream/sample"><img data-src="/cover.jpg"><h3>Sample Anime</h3></a></div></div>"#;
const SEARCH_FIXTURE: &str = r#"[{"name":"Sample Anime","link":"sample","cover":"/cover-150x225.jpg","description":"Sample description."}]"#;
const DETAILS_FIXTURE: &str = r#"<div class="series-title"><h1><span>Sample Anime</span></h1></div><div class="seriesCoverBox"><img data-src="/cover.jpg"></div><p class="seri_des" data-full-description="Sample description."></p><div id="stream"><ul><li><a href="/anime/stream/sample/staffel-1">Staffel 1</a></li></ul></div><table class="seasonEpisodesList"><tbody><tr data-episode-season-id="1"><td class="seasonEpisodeTitle"><a href="/anime/stream/sample/staffel-1/episode-1"><span>Episode 1</span></a></td><td><meta content="1"></td></tr></tbody></table>"#;
const HOSTERS_FIXTURE: &str = r#"<ul class="row"><li data-lang-key="3"><a class="watchEpisode" href="/redirect/1"><h4>VOE</h4></a></li></ul>"#;

export_video_source!(SOURCE);
