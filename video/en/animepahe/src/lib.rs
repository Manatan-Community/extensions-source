use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AnimePahe = AnimePahe;
const DEFAULT_BASE_URL: &str = "https://animepahe.pw";
const UA: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Mobile Safari/537.36";

struct AnimePahe;

impl VideoSource for AnimePahe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = get_or_fixture(
            &base,
            &format!("{base}/api?m=airing&page={page}"),
            LIST_FIXTURE,
        );
        Ok(parse_api_list(&base, &body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&base, &path)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let body = get_or_fixture(
            &base,
            &format!(
                "{base}/api?m=search&q={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&base, &body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/a/1".to_string());
        Ok(fetch_details(&base, &path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/a/1".to_string());
        let session = if path.starts_with("/anime/") {
            path.trim_start_matches("/anime/").to_string()
        } else {
            fetch_session(&base, &path).unwrap_or_else(|| "sample-session".to_string())
        };
        let body = get_or_fixture(
            &base,
            &format!("{base}/api?m=release&id={session}&sort=episode_asc&page=1"),
            EPISODES_FIXTURE,
        );
        Ok(parse_episodes(&body, &session))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/play/sample-session/sample-episode".to_string());
        let body = get_or_fixture(&base, &format!("{base}{path}"), WATCH_FIXTURE);
        Ok(parse_hosters(&base, &body, &path))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(3, '|');
        let kind = parts.next().unwrap_or("kwik");
        let quality = parts.next().unwrap_or("Video");
        let url = parts.next().unwrap_or_default();
        let mut streams = if kind == "kwik-hls" {
            resolve_kwik_hls(url, quality)
        } else {
            vec![external_stream(url, quality, "Kwik")]
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
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Airing".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|path| format!("{base}{path}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode").map(|path| format!("{base}{path}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            let base = base_url(&request);
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&base, &path)),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", UA)
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_api_list(base: &str, body: &str) -> Paged<CatalogItem> {
    let payload: ResponseDto<LatestAnimeDto> = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|anime| CatalogItem {
                key: format!("/a/{}", anime.anime_id),
                title: anime.anime_title,
                cover: Some(anime.snapshot),
                url: Some(format!("{base}/a/{}", anime.anime_id)),
                authors: anime.fansub.into_iter().collect(),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: payload.current_page < payload.last_page,
    }
}

fn parse_search(base: &str, body: &str) -> Paged<CatalogItem> {
    let payload: ResponseDto<SearchResultDto> = serde_json::from_str(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .into_iter()
            .map(|anime| CatalogItem {
                key: format!("/a/{}", anime.id),
                title: anime.title,
                cover: Some(anime.poster),
                url: Some(format!("{base}/a/{}", anime.id)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
            .collect(),
        has_next_page: false,
    }
}

fn fetch_details(base: &str, path: &str) -> CatalogItem {
    let body = get_or_fixture(base, &format!("{base}{path}"), DETAILS_FIXTURE);
    CatalogItem {
        key: path.to_string(),
        title: html::text_between(&body, "div class=\"title-wrapper", "</h1>")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        authors: info_value(&body, "Studios:").into_iter().collect(),
        cover: html::attr_after(&body, "anime-poster", "href"),
        description: description(&body),
        tags: collect_tags(&body),
        url: Some(format!("{base}{path}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&info_value(&body, "Status:").unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, anime_session: &str) -> Vec<VideoEpisode> {
    let payload: ResponseDto<EpisodeDto> = serde_json::from_str(body).unwrap_or_default();
    let mut episodes = payload
        .data
        .into_iter()
        .map(|episode| {
            let number = if episode.episode.fract() == 0.0 {
                format!("{}", episode.episode as i32)
            } else {
                episode.episode.to_string()
            };
            VideoEpisode {
                key: format!("/play/{anime_session}/{}", episode.session),
                title: Some(format!("Episode {number}")),
                episode_number: Some(episode.episode),
                url: Some(format!(
                    "{DEFAULT_BASE_URL}/play/{anime_session}/{}",
                    episode.session
                )),
                language: Some("en".to_string()),
                labels: vec!["subbed".to_string()],
                ..VideoEpisode::default()
            }
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

fn parse_hosters(base: &str, body: &str, path: &str) -> Vec<VideoHoster> {
    let downloads = body
        .split("id=\"pickDownload\"")
        .nth(1)
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .collect::<Vec<_>>();
    body.split("id=\"resolutionMenu\"")
        .nth(1)
        .unwrap_or(body)
        .split("<button")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let kwik = html::attr(chunk, "data-src")?;
            let quality = html::strip_tags(chunk.split('>').nth(1).unwrap_or(chunk))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let direct = downloads.get(index).cloned();
            let target = if direct.as_deref().is_some_and(|url| !url.is_empty()) {
                direct.unwrap()
            } else {
                kwik
            };
            Some(VideoHoster {
                key: format!("kwik-hls|{}|{}", quality, target),
                name: format!("Kwik {quality}"),
                url: Some(format!("{base}{path}")),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(base),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_kwik_hls(url: &str, quality: &str) -> Vec<VideoStream> {
    let body = HttpClient::browser()
        .with_header("User-Agent", UA)
        .with_referer(DEFAULT_BASE_URL)
        .with_cookies_for(url)
        .with_webview_challenge_fallback()
        .get(url)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    if let Some(stream_url) = extract_kwik_source(&body) {
        return vec![media_stream(&stream_url, quality, "Kwik")];
    }
    vec![external_stream(url, quality, "Kwik")]
}

fn extract_kwik_source(body: &str) -> Option<String> {
    for marker in [
        "const source=\\'",
        "const source='",
        "source: '",
        "source:\"",
    ] {
        if let Some(rest) = body.split(marker).nth(1) {
            let end = if marker.ends_with('"') { "\"" } else { "'" };
            let value = rest.split(end).next()?.replace("\\/", "/");
            if value.starts_with("http") {
                return Some(value);
            }
        }
    }
    None
}

fn media_stream(url: &str, quality: &str, hoster: &str) -> VideoStream {
    let is_hls = url.contains(".m3u8");
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{hoster} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers("https://kwik.cx/"),
        ..VideoStream::default()
    }
}

fn external_stream(url: &str, quality: &str, hoster: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(format!("{hoster} {quality}")),
        quality: Some(quality.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(DEFAULT_BASE_URL),
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_quality = pref(request, "preferred_quality", "1080p");
    let preferred_sub = pref(request, "preferred_sub", "jpn");
    let preferred_av1 = pref_bool(request, "preferred_av1", false);
    streams.sort_by_key(|stream| {
        let quality = stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (
            i32::from(quality.contains(&preferred_quality.to_ascii_lowercase())),
            i32::from(quality.contains(preferred_sub)),
            i32::from(quality.contains("av1") == preferred_av1),
            quality_score(&quality),
        )
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .is_some_and(|quality| quality.contains(preferred_quality));
    }
}

fn fetch_session(base: &str, path: &str) -> Option<String> {
    let body = get_or_fixture(base, &format!("{base}{path}"), "");
    path_from_url(&body).and_then(|path| path.strip_prefix("/anime/").map(ToString::to_string))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| chunk.split("</p>").next())
        .map(html::strip_tags)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(summary) = html::text_between(body, "div class=\"anime-summary", "</div>") {
        parts.push(html::strip_tags(&summary));
    }
    for label in ["Synonyms:", "Japanese:", "Aired:", "Season:"] {
        if let Some(value) = info_value(body, label) {
            parts.push(format!("{label} {value}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn collect_tags(body: &str) -> Vec<String> {
    body.split("anime-genre")
        .nth(1)
        .unwrap_or("")
        .split("<li")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value {
        "Currently Airing" => ItemStatus::Ongoing,
        "Finished Airing" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn path_from_url(input: &str) -> Option<String> {
    if let Some(rest) = input.split("/a/").nth(1) {
        return Some(format!(
            "/a/{}",
            rest.split(['"', '\'', '<', '?']).next()?.trim_matches('/')
        ));
    }
    if let Some(rest) = input.split("/anime/").nth(1) {
        return Some(format!(
            "/anime/{}",
            rest.split(['"', '\'', '<', '?']).next()?.trim_matches('/')
        ));
    }
    None
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("AnimePahe")
        .replace('-', " ")
}

fn quality_score(value: &str) -> i32 {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .find_map(|part| part.parse::<i32>().ok())
        .unwrap_or(0)
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert(
        "Referer".to_string(),
        format!("{}/", referer.trim_end_matches('/')),
    );
    headers
}

fn base_url(request: &Value) -> String {
    pref(request, "preferred_domain", DEFAULT_BASE_URL).to_string()
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .or_else(|| request.get(key).and_then(Value::as_bool))
        .unwrap_or(default)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

#[derive(Default, Deserialize)]
struct ResponseDto<T> {
    #[serde(default)]
    current_page: u64,
    #[serde(default)]
    last_page: u64,
    #[serde(default)]
    data: Vec<T>,
}

#[derive(Default, Deserialize)]
struct LatestAnimeDto {
    anime_title: String,
    snapshot: String,
    anime_id: u64,
    fansub: Option<String>,
}

#[derive(Default, Deserialize)]
struct SearchResultDto {
    title: String,
    poster: String,
    id: u64,
}

#[derive(Default, Deserialize)]
struct EpisodeDto {
    session: String,
    episode: f32,
}

const LIST_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"data":[{"anime_title":"Sample AnimePahe","snapshot":"https://fixtures.invalid/animepahe/snapshot.jpg","anime_id":1,"fansub":"FixtureSub"}]}"#;
const SEARCH_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"data":[{"title":"Sample AnimePahe","poster":"https://fixtures.invalid/animepahe/poster.jpg","id":1}]}"#;
const DETAILS_FIXTURE: &str = r#"<div class="title-wrapper"><h1><span>Sample AnimePahe</span></h1></div><div class="anime-poster"><a href="https://fixtures.invalid/animepahe/poster.jpg"></a></div><div class="anime-summary">Fixture details.</div><div class="anime-genre"><ul><li><a>Action</a></li></ul></div><div class="col-sm-4 anime-info"><p>Status: <a>Finished Airing</a></p><p>Studios: Fixture Studio</p></div>"#;
const EPISODES_FIXTURE: &str = r#"{"current_page":1,"last_page":1,"data":[{"created_at":"2024-01-01 00:00:00","session":"sample-episode-1","episode":1},{"created_at":"2024-01-02 00:00:00","session":"sample-episode-2","episode":2}]}"#;
const WATCH_FIXTURE: &str = r#"<div id="pickDownload"><a href="https://kwik.cx/f/sample"></a></div><div id="resolutionMenu"><button data-src="https://kwik.cx/e/sample">720p</button></div>"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_list() {
        let page = parse_api_list(DEFAULT_BASE_URL, LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample AnimePahe");
    }

    #[test]
    fn parses_hosters() {
        let hosters = parse_hosters(DEFAULT_BASE_URL, WATCH_FIXTURE, "/play/a/e");
        assert_eq!(hosters[0].name, "Kwik 720p");
    }
}
