use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: MoflixStream = MoflixStream;
const BASE_URL: &str = "https://moflix-stream.xyz";
const API_URL: &str = "https://moflix-stream.xyz/api/v1";
const TITLE_QUERIES: &str =
    "load=images,genres,productionCountries,keywords,videos,primaryVideo,seasons,compactCredits";

struct MoflixStream;

impl VideoSource for MoflixStream {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = get_or_fixture(
            &format!("{API_URL}/channel/345?returnContentOnly=true&restriction=&order=rating:desc&paginate=simple&perPage=50&query=&page={page}"),
            LIST_FIXTURE,
            &format!("{BASE_URL}/movies?order=rating%3Adesc"),
        );
        Ok(parse_popular(&body))
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
        let encoded = manatan_shared::sdk::http::url_encode(query);
        let body = get_or_fixture(
            &format!("{API_URL}/search/{encoded}?query={encoded}"),
            SEARCH_FIXTURE,
            &format!("{BASE_URL}/search/{encoded}"),
        );
        let json: Value = serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
        Ok(Paged {
            entries: json
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(item_from_info)
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| format!("/titles/sample?{TITLE_QUERIES}"));
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| format!("/titles/sample?{TITLE_QUERIES}"));
        let body = get_or_fixture(&api_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body, &path))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode").unwrap_or_else(|| format!("/titles/sample?{TITLE_QUERIES}"));
        let body = get_or_fixture(&api_url(&path), VIDEOS_FIXTURE, BASE_URL);
        let selected = selected_hosters(&request);
        Ok(parse_video_hosters(&body)
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
        let mut streams = vec![external_stream(&key, name, BASE_URL)];
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://streamtape"),
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
            pref(&request, "preferred_hoster", "https://streamtape"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Movies".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| api_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| api_url(&path)))
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client()
        .get(target)
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let pagination = json.get("pagination").unwrap_or(&Value::Null);
    let current = pagination.get("current_page").and_then(Value::as_u64).unwrap_or(1);
    let next = pagination.get("next_page").and_then(Value::as_u64).unwrap_or(current);
    Paged {
        entries: pagination
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(item_from_info)
            .collect(),
        has_next_page: current < next,
    }
}

fn item_from_info(item: &Value) -> CatalogItem {
    let id = value_as_string(item.get("id").unwrap_or(&Value::Null));
    CatalogItem {
        key: format!("/titles/{id}?{TITLE_QUERIES}"),
        title: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Moflix-Stream")
            .to_string(),
        cover: item
            .get("poster")
            .or_else(|| item.get("backdrop"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(format!("{API_URL}/titles/{id}?{TITLE_QUERIES}")),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&api_url(path), DETAILS_FIXTURE, BASE_URL);
    let json: Value = serde_json::from_str(&body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let title = json.get("title").unwrap_or(&json);
    CatalogItem {
        key: path_key(path),
        title: title
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Moflix-Stream")
            .to_string(),
        cover: title
            .get("poster")
            .or_else(|| title.get("backdrop"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(api_url(path)),
        description: title
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: title
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("display_name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, path: &str) -> Vec<VideoEpisode> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let title_id = value_as_string(json.get("title").and_then(|title| title.get("id")).unwrap_or(&Value::Null));
    let Some(seasons) = json.get("seasons").and_then(|seasons| seasons.get("data")).and_then(Value::as_array) else {
        return vec![VideoEpisode {
            key: path_key(path),
            title: Some("Film".to_string()),
            episode_number: Some(1.0),
            url: Some(api_url(path)),
            language: Some("de".to_string()),
            ..VideoEpisode::default()
        }];
    };
    let mut out = Vec::new();
    for season in seasons {
        let season_num = season.get("number").and_then(Value::as_u64).unwrap_or(1);
        let episodes_url = format!("{API_URL}/titles/{title_id}/seasons/{season_num}?load=episodes,primaryVideo");
        let season_body = get_or_fixture(&episodes_url, EPISODES_FIXTURE, BASE_URL);
        let season_json: Value = serde_json::from_str(&season_body).unwrap_or_else(|_| serde_json::from_str(EPISODES_FIXTURE).unwrap());
        for ep in season_json
            .get("episodes")
            .and_then(|episodes| episodes.get("data"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .rev()
        {
            let ep_num = ep.get("episode_number").and_then(Value::as_f64).unwrap_or(1.0);
            let name = ep.get("name").and_then(Value::as_str).unwrap_or_default();
            out.push(VideoEpisode {
                key: format!("/titles/{title_id}/seasons/{season_num}/episodes/{}?load=videos,compactCredits,primaryVideo", ep_num as u64),
                title: Some(format!("Staffel {season_num} Folge {} : {name}", ep_num as u64)),
                episode_number: Some(ep_num as f32),
                season_number: Some(season_num as f32),
                url: Some(format!("{API_URL}/titles/{title_id}/seasons/{season_num}/episodes/{}?load=videos,compactCredits,primaryVideo", ep_num as u64)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    out
}

fn parse_video_hosters(body: &str) -> Vec<VideoHoster> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(VIDEOS_FIXTURE).unwrap());
    let videos = json
        .get("episode")
        .or_else(|| json.get("title"))
        .and_then(|value| value.get("videos"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    videos
        .filter_map(|video| {
            let src = video.get("src").and_then(Value::as_str)?;
            let name = video.get("name").and_then(Value::as_str).unwrap_or("Mirror");
            Some(VideoHoster {
                key: src.to_string(),
                name: name.to_string(),
                url: Some(src.to_string()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
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
        return vec!["streamtape".to_string(), "vidguard".to_string(), "streamvid".to_string(), "highstream".to_string(), "filelions".to_string(), "lulustream".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|value| match value {
            "stape" => "streamtape",
            "vidg" => "vidguard",
            "svid" => "streamvid",
            "hstream" => "highstream",
            "flions" => "filelions",
            "lstream" => "lulustream",
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
        stream.preferred = stream.url.contains(preferred_hoster);
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
    if input.starts_with(API_URL) || input.starts_with(BASE_URL) || input.starts_with("/titles/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(API_URL) && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    input
        .strip_prefix(API_URL)
        .or_else(|| input.strip_prefix(BASE_URL))
        .unwrap_or(input)
        .to_string()
}

fn api_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        url::join_url(API_URL, path)
    }
}

fn value_as_string(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_i64().map(|num| num.to_string()))
        .or_else(|| value.as_u64().map(|num| num.to_string()))
        .unwrap_or_default()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

const LIST_FIXTURE: &str = r#"{
  "pagination": { "current_page": 1, "next_page": null, "data": [{ "id": "sample", "name": "Sample", "poster": "https://moflix-stream.xyz/sample.jpg" }] }
}"#;

const SEARCH_FIXTURE: &str = r#"{ "results": [{ "id": "sample", "name": "Sample", "poster": "https://moflix-stream.xyz/sample.jpg" }] }"#;

const DETAILS_FIXTURE: &str = r#"{
  "title": { "id": "sample", "name": "Sample", "description": "Sample", "poster": "https://moflix-stream.xyz/sample.jpg", "genres": [{ "display_name": "Action" }] },
  "seasons": null
}"#;

const EPISODES_FIXTURE: &str = r#"{ "episodes": { "data": [{ "name": "Pilot", "episode_number": 1 }] } }"#;

const VIDEOS_FIXTURE: &str = r#"{
  "title": { "videos": [{ "name": "Streamtape", "src": "https://streamtape.com/e/sample" }] }
}"#;

export_video_source!(SOURCE);
