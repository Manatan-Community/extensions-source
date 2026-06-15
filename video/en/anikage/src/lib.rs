use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind,
    abi::{ExtensionResult, system_time},
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
};
use serde_json::{Value, json};

const SOURCE: Anikage = Anikage;
const BASE_URL: &str = "https://anikage.cc";
const ANILIST_API: &str = "https://graphql.anilist.co";
const API_KEY_DEFAULT: &str = "x9f2k7m4q1w8e3r6t5y0";
const PAGE_SIZE: u64 = 30;
const SUB_PROVIDERS: [&str; 12] = [
    "uwu", "beep", "mochi", "miku", "mimi", "vee", "kiwi", "yuki", "kami", "shiro", "wave",
    "zaza",
];
const DUB_PROVIDERS: [&str; 7] = ["mochi", "miku", "mimi", "kiwi", "yuki", "uwu", "kami"];

struct Anikage;

impl VideoSource for Anikage {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "ID_DESC"
        } else {
            "TRENDING_DESC"
        };
        let body = anilist_post(json!({
            "type": "ANIME",
            "page": page,
            "perPage": PAGE_SIZE,
            "sort": [sort],
            "isAdult": pref_bool(&request, "nsfw", false)
        }));
        Ok(parse_page(&body, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&id, &request)],
                has_next_page: false,
            });
        }
        let mut variables = serde_json::Map::new();
        variables.insert("type".to_string(), json!("ANIME"));
        variables.insert("page".to_string(), json!(page(&request)));
        variables.insert("perPage".to_string(), json!(PAGE_SIZE));
        variables.insert("sort".to_string(), json!([filter(&request, "sort", "TRENDING_DESC")]));
        variables.insert("isAdult".to_string(), json!(pref_bool(&request, "nsfw", false)));
        if !query.is_empty() {
            variables.insert("search".to_string(), json!(query));
        }
        for (filter_key, api_key) in [("season", "season"), ("origin", "countryOfOrigin")] {
            let value = filter(&request, filter_key, "ALL");
            if value != "ALL" {
                variables.insert(api_key.to_string(), json!(value));
            }
        }
        let release_year = filter(&request, "release_year", "ALL");
        if let Ok(year) = release_year.parse::<u16>() {
            variables.insert("seasonYear".to_string(), json!(year));
        }
        let format = filter(&request, "type", "ALL");
        if format != "ALL" {
            variables.insert("format_in".to_string(), json!([format]));
        }
        let genres = array_filter(&request, "genres");
        if !genres.is_empty() {
            variables.insert("genre_in".to_string(), json!(genres));
        }
        let body = anilist_post(Value::Object(variables));
        Ok(parse_page(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        let id = id_from_key(&key);
        let token = make_token(id, pref(&request, "private_api_key", API_KEY_DEFAULT), false);
        let target = format!("{BASE_URL}/api/anime/episodes/{token}");
        let body = client()
            .get(&target)
            .header("Accept", "*/*")
            .header("Origin", BASE_URL)
            .referer(&format!("{BASE_URL}/anime/info/{id}"))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| EPISODES_FIXTURE.to_string());
        let provider = provider(&request);
        let kind = pref(&request, "is_sub_or_dub", "sub");
        let episodes = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_else(|| serde_json::from_str::<Value>(EPISODES_FIXTURE).unwrap().as_array().unwrap().clone())
            .into_iter()
            .filter_map(|episode| {
                let number = episode.get("number").and_then(Value::as_u64)?;
                let title = episode.get("title").and_then(Value::as_str);
                let key = json!({
                    "id": id,
                    "provider": provider,
                    "episode": number,
                    "kind": kind
                })
                .to_string();
                Some(VideoEpisode {
                    key,
                    title: Some(title.map_or_else(|| format!("Episode {number}"), |title| format!("Episode {number} - {title}"))),
                    episode_number: Some(number as f32),
                    thumbnail: episode.get("img").and_then(Value::as_str).map(ToString::to_string),
                    language: Some("en".to_string()),
                    labels: vec![kind.to_string(), provider.to_string()],
                    url: Some(format!("{BASE_URL}/anime/watch/{id}?host={provider}&ep={number}&type={kind}")),
                    ..VideoEpisode::default()
                })
            })
            .collect();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode")
            .unwrap_or_else(|| json!({"id":1,"provider":"uwu","episode":1,"kind":"sub"}).to_string());
        let data: Value = serde_json::from_str(&key).unwrap_or_else(|_| json!({}));
        let id = data.get("id").and_then(Value::as_u64).unwrap_or(1);
        let episode = data.get("episode").and_then(Value::as_u64).unwrap_or(1);
        let kind = data.get("kind").and_then(Value::as_str).unwrap_or("sub");
        let primary = data
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_else(|| provider(&request));
        let mut providers = provider_order(kind, primary);
        if !pref_bool(&request, "try_fallback_providers", true) {
            providers.truncate(1);
        }
        let mut streams = Vec::new();
        for provider in providers {
            let token = make_sources_token(
                id,
                episode,
                provider,
                kind,
                pref(&request, "private_api_key", API_KEY_DEFAULT),
            );
            let target = format!("{BASE_URL}/api/anime/sources/{token}");
            let body = client()
                .get(&target)
                .header("Accept", "*/*")
                .header("Origin", BASE_URL)
                .referer(&format!("{BASE_URL}/anime/watch/{id}?host={provider}&ep={episode}&type={kind}"))
                .xhr()
                .send_text()
                .unwrap_or_else(|_| SOURCES_FIXTURE.to_string());
            streams.extend(parse_streams(&body, kind, provider, &target));
            if !streams.is_empty() {
                break;
            }
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
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| format!("{BASE_URL}/anime/info/{}", id_from_key(&key))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode")
            .and_then(|key| serde_json::from_str::<Value>(&key).ok())
            .map(|data| {
                format!(
                    "{BASE_URL}/anime/watch/{}?host={}&ep={}&type={}",
                    data.get("id").and_then(Value::as_u64).unwrap_or(1),
                    data.get("provider").and_then(Value::as_str).unwrap_or("uwu"),
                    data.get("episode").and_then(Value::as_u64).unwrap_or(1),
                    data.get("kind").and_then(Value::as_str).unwrap_or("sub")
                )
            }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id, &request)),
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
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn anilist_post(variables: Value) -> String {
    client()
        .post(ANILIST_API)
        .header("Accept", "*/*")
        .header("Origin", BASE_URL)
        .referer(ANILIST_API)
        .json(json!({ "variables": variables, "query": ANILIST_QUERY }).to_string())
        .send_text()
        .unwrap_or_else(|_| PAGE_FIXTURE.to_string())
}

fn fetch_details(key: &str, request: &Value) -> CatalogItem {
    let id = id_from_key(key);
    let body = client()
        .post(ANILIST_API)
        .header("Accept", "*/*")
        .header("Origin", BASE_URL)
        .referer(ANILIST_API)
        .json(json!({ "variables": { "id": id }, "query": DETAILS_QUERY }).to_string())
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let media = root.pointer("/data/Media").unwrap_or(&Value::Null);
    media_item(media, request).unwrap_or_else(|| CatalogItem {
        key: id.to_string(),
        title: format!("Anikage {id}"),
        url: Some(format!("{BASE_URL}/anime/info/{id}")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_page(body: &str, request: &Value) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let page = root.pointer("/data/Page").unwrap_or(&Value::Null);
    let entries = page
        .get("media")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| media_item(value, request))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: page
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        entries,
    }
}

fn media_item(media: &Value, request: &Value) -> Option<CatalogItem> {
    let id = media.get("id").and_then(Value::as_u64)?;
    let title_data = media.get("title").unwrap_or(&Value::Null);
    let romaji = title_data
        .get("romaji")
        .and_then(Value::as_str)
        .unwrap_or("Anikage");
    let title = if pref(request, "title_format", "english") == "english" {
        title_data
            .get("english")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(romaji)
    } else {
        romaji
    };
    Some(CatalogItem {
        key: id.to_string(),
        title: title.to_string(),
        cover: media
            .pointer("/coverImage/extraLarge")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        banner: media.get("bannerImage").and_then(Value::as_str).map(ToString::to_string),
        url: Some(format!("{BASE_URL}/anime/info/{id}")),
        description: media
            .get("description")
            .and_then(Value::as_str)
            .map(html::strip_tags),
        tags: media
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        rating: media
            .get("averageScore")
            .and_then(Value::as_f64)
            .map(|score| (score / 20.0) as f32),
        language: Some("en".to_string()),
        content_rating: Some(if media.get("isAdult").and_then(Value::as_bool).unwrap_or(false) {
            "adult"
        } else {
            "safe"
        }.to_string()),
        status: parse_status(media.get("status").and_then(Value::as_str)),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_streams(body: &str, kind: &str, provider: &str, referer: &str) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(SOURCES_FIXTURE).unwrap());
    let headers = stream_headers(root.get("headers").unwrap_or(&Value::Null), referer);
    let subtitles = root
        .get("subtitles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|subtitle| {
            Some(SubtitleTrack {
                url: subtitle.get("url")?.as_str()?.to_string(),
                language: subtitle.get("lang").and_then(Value::as_str).map(ToString::to_string),
                label: subtitle.get("label").and_then(Value::as_str).map(ToString::to_string),
                format: Some("vtt".to_string()),
                is_default: subtitle.get("default").and_then(Value::as_bool).unwrap_or(false),
                ..SubtitleTrack::default()
            })
        })
        .collect::<Vec<_>>();
    root.get("sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|source| {
            let url = source.get("url")?.as_str()?.to_string();
            let quality = source.get("quality").and_then(Value::as_str).unwrap_or("auto");
            let is_hls = url.contains(".m3u8");
            Some(VideoStream {
                url,
                name: Some(format!("{kind} - {provider} - {quality}")),
                quality: Some(quality.to_string()),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls { VideoStreamKind::Hls } else { VideoStreamKind::Direct }),
                headers: headers.clone(),
                subtitles: subtitles.clone(),
                initialized: true,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn stream_headers(value: &Value, referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), value.get("Referer").and_then(Value::as_str).unwrap_or(referer).to_string());
    if let Some(origin) = value.get("Origin").and_then(Value::as_str) {
        headers.insert("Origin".to_string(), origin.to_string());
    }
    if let Some(user_agent) = value.get("User-Agent").and_then(Value::as_str) {
        headers.insert("User-Agent".to_string(), user_agent.to_string());
    }
    headers
}

fn make_token(id: u64, api_key: &str, refresh: bool) -> String {
    xor_token(json!({ "id": id, "refresh": refresh.to_string(), "_t": unix_time().to_string() }).to_string(), api_key)
}

fn make_sources_token(id: u64, episode: u64, provider: &str, kind: &str, api_key: &str) -> String {
    xor_token(
        json!({ "id": id, "epNum": episode, "host": provider, "type": kind, "_t": unix_time().to_string() }).to_string(),
        api_key,
    )
}

fn xor_token(payload: String, api_key: &str) -> String {
    let key = api_key.as_bytes();
    let out = payload
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ key[index % key.len()])
        .collect::<Vec<_>>();
    URL_SAFE_NO_PAD.encode(out)
}

fn unix_time() -> u64 {
    system_time()
        .map(|time| time.unix_seconds.max(0) as u64)
        .unwrap_or(0)
}

fn provider(request: &Value) -> &str {
    if pref(request, "is_sub_or_dub", "sub") == "dub" {
        pref(request, "preferred_dub_source", "miku")
    } else {
        pref(request, "preferred_sub_source", "uwu")
    }
}

fn provider_order<'a>(kind: &str, primary: &'a str) -> Vec<&'a str> {
    let base = if kind == "dub" { &DUB_PROVIDERS[..] } else { &SUB_PROVIDERS[..] };
    let mut out = vec![primary];
    out.extend(base.iter().copied().filter(|candidate| *candidate != primary));
    out
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("FINISHED") => ItemStatus::Completed,
        Some("RELEASING") | Some("NOT_YET_RELEASED") => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|stream| {
        stream
            .quality
            .as_deref()
            .unwrap_or_default()
            .contains(preferred)
    });
    streams.reverse();
}

fn id_from_url(input: &str) -> Option<String> {
    input
        .split("/anime/info/")
        .nth(1)
        .or_else(|| input.split("/anime/watch/").nth(1))
        .map(|value| value.split(['/', '?', '#']).next().unwrap_or(value).to_string())
}

fn id_from_key(key: &str) -> u64 {
    key.split(['/', '?', '#'])
        .next_back()
        .unwrap_or(key)
        .parse()
        .unwrap_or(1)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| request.get("key").and_then(Value::as_str).map(ToString::to_string))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn filter<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .or_else(|| request.get(key).and_then(Value::as_str))
        .unwrap_or(default)
}

fn array_filter(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

const ANILIST_QUERY: &str = r#"query($page:Int=1,$perPage:Int=30,$type:MediaType=ANIME,$search:String,$format_in:[MediaFormat],$status:MediaStatus,$countryOfOrigin:CountryCode,$season:MediaSeason,$seasonYear:Int,$genre_in:[String],$sort:[MediaSort],$isAdult:Boolean){Page(page:$page,perPage:$perPage){pageInfo{total perPage currentPage lastPage hasNextPage}media(type:$type,sort:$sort,season:$season,seasonYear:$seasonYear,search:$search,genre_in:$genre_in,format_in:$format_in,status:$status,countryOfOrigin:$countryOfOrigin,isAdult:$isAdult){id title{english romaji}coverImage{extraLarge color}startDate{year month day}bannerImage season seasonYear description type format status(version:2)episodes duration genres isAdult averageScore popularity nextAiringEpisode{airingAt timeUntilAiring episode}}}}"#;
const DETAILS_QUERY: &str = r#"query($id:Int!){Media(id:$id,type:ANIME){id title{english romaji}coverImage{extraLarge color}bannerImage description type format status(version:2)episodes duration genres isAdult averageScore popularity}}"#;
const PAGE_FIXTURE: &str = r#"{"data":{"Page":{"pageInfo":{"hasNextPage":false},"media":[{"id":1,"title":{"english":"Sample Anikage","romaji":"Sample Anikage"},"coverImage":{"extraLarge":"https://fixtures.invalid/cover.jpg","color":null},"bannerImage":null,"description":"Fixture details.","type":"ANIME","format":"TV","status":"FINISHED","genres":["Action"],"isAdult":false,"averageScore":80}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"Media":{"id":1,"title":{"english":"Sample Anikage","romaji":"Sample Anikage"},"coverImage":{"extraLarge":"https://fixtures.invalid/cover.jpg","color":null},"bannerImage":null,"description":"Fixture details.","type":"ANIME","format":"TV","status":"FINISHED","genres":["Action"],"isAdult":false,"averageScore":80}}}"#;
const EPISODES_FIXTURE: &str = r#"[{"number":1,"title":"Pilot","description":"","img":"https://fixtures.invalid/episode.jpg","isFiller":false,"subProviders":["uwu"],"dubProviders":["miku"]}]"#;
const SOURCES_FIXTURE: &str = r#"{"sources":[{"url":"https://fixtures.invalid/video-720.mp4","quality":"720p"}],"subtitles":[{"id":"en","url":"https://fixtures.invalid/en.vtt","lang":"en","label":"English","kind":"captions","default":true}],"headers":{"Referer":"https://anikage.cc","Origin":"https://anikage.cc","User-Agent":null}}"#;

export_video_source!(SOURCE);
