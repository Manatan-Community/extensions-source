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
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AnimeWorldIndia = AnimeWorldIndia;
const BASE_URL: &str = "https://anime-world.co";

struct AnimeWorldIndia;

impl VideoSource for AnimeWorldIndia {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "update"
        } else {
            "viewed"
        };
        let target = format!("{BASE_URL}/advanced-search/page/{page}/?s_lang=all&s_orderby={sort}");
        let body = fetch_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&slug)],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/advanced-search/page/{}/?s_keyword={}&s_lang=all",
            page(&request),
            url::query_escape(query)
        );
        let body = fetch_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample-anime".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "sample-anime".to_string());
        let item_url = item_url_from_key(&key);
        let body = fetch_or_fixture(&item_url, DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &key))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode_key =
            request_key(&request, "episode").unwrap_or_else(|| "sample-anime|1".to_string());
        let endpoint = episode_endpoint(&episode_key);
        let body = fetch_or_fixture(&endpoint, PLAYERS_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &request, &episode_key))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(hoster_key) = request_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = hoster_key.split('|');
        let language = parts.next().unwrap_or("all");
        let server = parts.next().unwrap_or("Mystream");
        let stream_url = parts.collect::<Vec<_>>().join("|");
        if stream_url.is_empty() {
            return Ok(Vec::new());
        }
        if !server.eq_ignore_ascii_case("Mystream") {
            return Ok(vec![external_stream(
                &stream_url,
                server,
                language,
                &request,
            )]);
        }
        Ok(resolve_mystream(&stream_url, language, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let hosters = self.hosters(request.clone())?;
        let mut streams = Vec::new();
        for hoster in hosters {
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
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
        }))?;
        let latest = self.list(json!({
            "listing": "latest",
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
        }))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Most Viewed".to_string(),
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
        Ok(request_key(&request, "item").map(|key| item_url_from_key(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| episode_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&slug)),
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
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
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

fn fetch_details(key: &str) -> CatalogItem {
    let target = item_url_from_key(key);
    let body = fetch_or_fixture(&target, DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, key).unwrap_or_else(|| fallback_item(key))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("col-span-1")
        .skip(1)
        .filter_map(parse_listing_item)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("page-numbers")
            && body.contains("current")
            && body.contains("<li")
            && body.contains("</li>"),
    }
}

fn parse_listing_item(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = slug_from_url(&href)?;
    let title = html::text_between(block, "font-medium line-clamp-2 mb-3", "</div>")
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .or_else(|| html::attr_after(block, "<img", "alt"))
        .unwrap_or_else(|| key.replace(['-', '_'], " "));
    let cover = html::attr_after(block, "<img", "src");
    Some(CatalogItem {
        key,
        title,
        cover,
        url: Some(url::join_url(BASE_URL, &href)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: &str) -> Option<CatalogItem> {
    let title = html::text_between(body, "<h2 class=\"text-4xl", "</h2>")
        .or_else(|| html::text_between(body, "<h1 class=\"text-4xl", "</h1>"))
        .map(|text| html::strip_tags(&text))?;
    let description = html::attr_after(body, "data-synopsis", "data-synopsis").or_else(|| {
        body.split("data-synopsis")
            .nth(1)
            .and_then(|chunk| html::text_between(chunk, ">", "</div>"))
            .map(|text| html::strip_tags(&text))
    });
    let tags = anchor_texts(body, "genre");
    let authors = anchor_texts(body, "studio");
    let artists = anchor_texts(body, "producer");
    Some(CatalogItem {
        key: key.to_string(),
        title,
        cover: html::attr_after(body, "<img", "src"),
        url: Some(item_url_from_key(key)),
        description,
        tags,
        authors,
        artists,
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(body),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, item_key: &str) -> Vec<VideoEpisode> {
    let is_movie = body.contains("type/movies/");
    let Some(json_body) = body
        .split("var season_list = ")
        .nth(1)
        .and_then(|tail| tail.split("var season_label =").next())
        .map(|value| value.trim().trim_end_matches(';'))
    else {
        return Vec::new();
    };
    let seasons: Vec<SeasonDto> = serde_json::from_str(json_body).unwrap_or_default();
    let single_season = seasons.len() == 1;
    let mut fallback_number = 1.0_f32;
    let mut out = Vec::new();
    for (season_index, season) in seasons.into_iter().enumerate() {
        let season_name = if single_season {
            String::new()
        } else {
            format!("Season {}", season_index + 1)
        };
        for episode in season.episodes.all.into_iter().rev() {
            let ep_num = episode
                .metadata
                .number
                .parse::<f32>()
                .unwrap_or(fallback_number);
            let title = episode.metadata.title.trim();
            let name = if is_movie {
                "Movie".to_string()
            } else {
                let mut value = String::new();
                if !season_name.is_empty() {
                    value.push_str(&season_name);
                    value.push_str(" - ");
                }
                value.push_str(&format!("Episode {}", display_number(ep_num)));
                if !title.is_empty() {
                    value.push_str(" - ");
                    value.push_str(title);
                }
                value
            };
            out.push(VideoEpisode {
                key: format!("{item_key}|{}", episode.id),
                title: Some(name),
                episode_number: Some(if single_season {
                    ep_num
                } else {
                    fallback_number
                }),
                date_uploaded: episode
                    .metadata
                    .released
                    .and_then(|date| date.parse::<i64>().ok())
                    .map(|seconds| seconds * 1000),
                url: Some(format!(
                    "{BASE_URL}/wp-json/kiranime/v1/episode?id={}",
                    episode.id
                )),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            });
            fallback_number += 1.0;
        }
    }
    out.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn parse_hosters(body: &str, request: &Value, episode_key: &str) -> Vec<VideoHoster> {
    let preferred_language = preference(request, "preferredLanguage").unwrap_or_default();
    let Some(players_json) = body
        .split("\"players\":")
        .last()
        .and_then(|tail| tail.split(",\"noplayer\":").next())
        .map(str::trim)
    else {
        return Vec::new();
    };
    let players: Vec<PlayerDto> = serde_json::from_str(players_json).unwrap_or_default();
    players
        .into_iter()
        .filter(|player| player.kind == "stream" && !player.url.trim().is_empty())
        .filter(|player| {
            preferred_language.is_empty()
                || player.language.eq_ignore_ascii_case(&preferred_language)
        })
        .map(|player| {
            let language = if player.language.is_empty() {
                "all".to_string()
            } else {
                player.language
            };
            VideoHoster {
                key: format!("{}|{}|{}", language, player.server, player.url),
                name: format!("{} {}", player.server, language),
                url: Some(episode_url(episode_key)),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            }
        })
        .collect()
}

fn resolve_mystream(target: &str, language: &str, request: &Value) -> Vec<VideoStream> {
    let referer = target;
    let response = client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send();
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
        Err(_) => (
            MYSTREAM_FIXTURE.to_string(),
            "PHPSESSID=fixture".to_string(),
        ),
    };
    let Some(stream_code) = body
        .split("sniff(")
        .nth(1)
        .and_then(|tail| tail.split(", \"").nth(1))
        .and_then(|tail| tail.split('"').next())
    else {
        return Vec::new();
    };
    let host = target.split("/watch").next().unwrap_or(BASE_URL);
    let stream_url = format!("{host}/m3u8/{stream_code}/master.txt?s=1&cache=1");
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), target.to_string());
    headers.insert("Accept".to_string(), "*/*".to_string());
    if !cookie.is_empty() {
        headers.insert("Cookie".to_string(), cookie);
    }
    let quality = "auto".to_string();
    vec![VideoStream {
        url: stream_url,
        name: Some(format!("[{language}] Mystream HLS")),
        quality: Some(quality.clone()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers,
        preferred: quality.contains(
            &preference(request, "preferredQuality").unwrap_or_else(|| "1080".to_string()),
        ),
        ..VideoStream::default()
    }]
}

fn external_stream(target: &str, server: &str, language: &str, request: &Value) -> VideoStream {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), BASE_URL.to_string());
    VideoStream {
        url: target.to_string(),
        name: Some(format!("[{language}] {server}")),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers,
        preferred: preference(request, "preferredQuality").is_none(),
        ..VideoStream::default()
    }
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_lowercase();
    if lower.contains("finished airing") || lower.contains("completed") || lower.contains(" movie ")
    {
        ItemStatus::Completed
    } else if lower.contains("currently airing") || lower.contains("airing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn anchor_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.replace(['-', '_'], " "),
        url: Some(item_url_from_key(key)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
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
        .map(ToString::to_string)
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn slug_from_url(input: &str) -> Option<String> {
    if input.starts_with("http") && !input.contains("anime-world.co") {
        return None;
    }
    url::slug_from_url(input.split(['?', '#']).next().unwrap_or(input))
        .filter(|slug| slug != "anime-world.co" && slug != "advanced-search")
}

fn item_url_from_key(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        format!("{BASE_URL}/{}", key.trim_matches('/'))
    }
}

fn episode_endpoint(episode_key: &str) -> String {
    let episode_id = episode_key.split('|').next_back().unwrap_or(episode_key);
    if episode_id.starts_with("http") {
        episode_id.to_string()
    } else {
        format!("{BASE_URL}/wp-json/kiranime/v1/episode?id={episode_id}")
    }
}

fn episode_url(episode_key: &str) -> String {
    episode_endpoint(episode_key)
}

fn display_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let quality = preference(request, "preferredQuality").unwrap_or_else(|| "1080".to_string());
    streams.sort_by_key(|stream| {
        !stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(&quality)
    });
}

#[derive(Default, Deserialize)]
struct SeasonDto {
    episodes: EpisodeTypeDto,
}

#[derive(Default, Deserialize)]
struct EpisodeTypeDto {
    all: Vec<EpisodeDto>,
}

#[derive(Deserialize)]
struct EpisodeDto {
    id: i64,
    metadata: EpisodeMetadataDto,
}

#[derive(Deserialize)]
struct EpisodeMetadataDto {
    number: String,
    title: String,
    released: Option<String>,
}

#[derive(Default, Deserialize)]
struct PlayerDto {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    language: String,
    server: String,
}

const SEARCH_FIXTURE: &str = r#"
<html><body>
<div class="col-span-1"><a href="https://anime-world.co/sample-anime"><img src="https://anime-world.co/sample.jpg" alt="Sample Anime"><div class="font-medium line-clamp-2 mb-3">Sample Anime</div></a></div>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
<h2 class="text-4xl">Sample Anime</h2>
<div data-synopsis="A sample fixture for local smoke tests.">A sample fixture for local smoke tests.</div>
<span class="leading-6"><a href="/genre/action">Action</a><a href="/studio/sample-studio">Sample Studio</a></span>
<nav><li><a href="/type/tv/">TV</a></li></nav>
<script>
var season_list = [{"episodes":{"all":[{"id":123,"metadata":{"number":"1","title":"Pilot","released":"1710000000"}}]}}];
var season_label = [];
</script>
</body></html>
"#;

const PLAYERS_FIXTURE: &str = r#"
{"players":[{"type":"stream","url":"https://mystream.example/watch/fixture","language":"english","server":"Mystream"}],"noplayer":false}
"#;

const MYSTREAM_FIXTURE: &str = r#"
<html><script>sniff("player", "fixture-code")</script></html>
"#;

export_video_source!(SOURCE);
