use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    dates, html,
    sdk::{SearchRequest, http::HttpClient},
};
use serde_json::{Map, Value, json};

const SOURCE: AniList = AniList;
const BASE_URL: &str = "https://anilist.co";
const API_URL: &str = "https://graphql.anilist.co";
const JIKAN_URL: &str = "https://api.jikan.moe/v4";
const PER_PAGE: u64 = 20;

struct AniList;

impl VideoSource for AniList {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let (sort, status) = if listing == "latest" {
            ("START_DATE_DESC", Some("RELEASING"))
        } else {
            ("TRENDING_DESC", None)
        };
        let mut variables = base_variables(&request);
        variables.insert("sort".to_string(), json!([sort]));
        if let Some(status) = status {
            variables.insert("status".to_string(), json!(status));
        }
        let body = anilist_post(SORT_QUERY, Value::Object(variables), PAGE_FIXTURE);
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

        let mut variables = base_variables(&request);
        variables.insert(
            "sort".to_string(),
            json!([filter(&request, "sort", "POPULARITY_DESC")]),
        );
        if !query.is_empty() {
            variables.insert("search".to_string(), json!(query));
        }

        let genres = string_array_filter(&request, "genres");
        if !genres.is_empty() {
            variables.insert("genres".to_string(), json!(genres));
        }

        let formats = string_array_filter(&request, "format");
        if !formats.is_empty() {
            variables.insert("format".to_string(), json!(formats));
        }

        let season = filter(&request, "season", "");
        let year = filter(&request, "year", "");
        if !season.is_empty() && !year.is_empty() {
            if let Ok(year) = year.parse::<u16>() {
                variables.insert("season".to_string(), json!(season));
                variables.insert("seasonYear".to_string(), json!(year));
            }
        } else if season.is_empty() && !year.is_empty() {
            variables.insert("year".to_string(), json!(format!("{year}%")));
        }

        for (filter_key, api_key) in [("status", "status"), ("country", "countryOfOrigin")] {
            let value = filter(&request, filter_key, "");
            if !value.is_empty() {
                variables.insert(api_key.to_string(), json!(value));
            }
        }

        let body = anilist_post(SORT_QUERY, Value::Object(variables), PAGE_FIXTURE);
        Ok(parse_page(&body, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&key, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "1".to_string());
        let id = id_from_key(&key);
        let body = anilist_post(
            EPISODE_QUERY,
            json!({ "id": id, "type": "ANIME" }),
            EPISODE_FIXTURE,
        );
        let root: Value = serde_json::from_str(&body).unwrap_or_default();
        let media = root.pointer("/data/Media").unwrap_or(&Value::Null);
        if media.get("status").and_then(Value::as_str) == Some("NOT_YET_RELEASED") {
            return Ok(Vec::new());
        }

        let episode_count = media
            .pointer("/nextAiringEpisode/episode")
            .and_then(Value::as_u64)
            .and_then(|episode| episode.checked_sub(1))
            .or_else(|| media.get("episodes").and_then(Value::as_u64))
            .unwrap_or(0);
        let mal_id = media.get("idMal").and_then(Value::as_u64);
        if let Some(mal_id) = mal_id {
            let episodes = episodes_from_mal(id, mal_id, episode_count, &request);
            if !episodes.is_empty() {
                return Ok(episodes);
            }
        }
        Ok(fallback_episodes(id, episode_count))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode")
            .unwrap_or_else(|| json!({ "id": 1, "episode": 1 }).to_string());
        let data: Value = serde_json::from_str(&key).unwrap_or_else(|_| json!({}));
        let id = data.get("id").and_then(Value::as_u64).unwrap_or(1);
        let episode = data.get("episode").and_then(Value::as_u64).unwrap_or(1);
        let url = format!("{BASE_URL}/anime/{id}");
        let _ = client()
            .get(&url)
            .browser_document()
            .referer(BASE_URL)
            .send_text()
            .ok();
        Ok(vec![VideoStream {
            url: url.clone(),
            name: Some(format!("AniList page - Episode {episode}")),
            quality: Some("Web".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            headers: referer_headers(BASE_URL),
            initialized: true,
            ..VideoStream::default()
        }])
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
        Ok(request_key(&request, "item").map(|key| anime_url(id_from_key(&key))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode")
            .and_then(|key| serde_json::from_str::<Value>(&key).ok())
            .and_then(|data| data.get("id").and_then(Value::as_u64).map(anime_url)))
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

fn anilist_post(query: &str, variables: Value, fixture: &str) -> String {
    client()
        .post(API_URL)
        .header("Accept", "application/json")
        .header("Origin", BASE_URL)
        .referer(BASE_URL)
        .json(json!({ "query": query, "variables": variables }).to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_variables(request: &Value) -> Map<String, Value> {
    let mut variables = Map::new();
    variables.insert("page".to_string(), json!(page(request)));
    variables.insert("perPage".to_string(), json!(PER_PAGE));
    variables.insert("type".to_string(), json!("ANIME"));
    if !pref_bool(request, "preferred_allow_adult", false) {
        variables.insert("isAdult".to_string(), json!(false));
    }
    variables
}

fn parse_page(body: &str, request: &Value) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let page = root.pointer("/data/Page").unwrap_or(&Value::Null);
    Paged {
        entries: page
            .get("media")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|media| media_item(media, request, false))
            .collect(),
        has_next_page: page
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn fetch_details(key: &str, request: &Value) -> CatalogItem {
    let id = id_from_key(key);
    let body = anilist_post(
        DETAILS_QUERY,
        json!({ "id": id, "type": "ANIME" }),
        DETAILS_FIXTURE,
    );
    let root: Value = serde_json::from_str(&body).unwrap_or_default();
    let media = root.pointer("/data/Media").unwrap_or(&Value::Null);
    media_item(media, request, true).unwrap_or_else(|| CatalogItem {
        key: id.to_string(),
        title: format!("AniList {id}"),
        url: Some(anime_url(id)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn media_item(media: &Value, request: &Value, initialized: bool) -> Option<CatalogItem> {
    let id = media.get("id").and_then(Value::as_u64)?;
    let title = title(media.get("title").unwrap_or(&Value::Null), request);
    let is_adult = media
        .get("isAdult")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(CatalogItem {
        key: id.to_string(),
        title,
        cover: cover(media),
        banner: media
            .get("bannerImage")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(anime_url(id)),
        description: initialized.then(|| description(media)).flatten(),
        tags: media
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        authors: main_studio(media).into_iter().collect(),
        rating: media
            .get("averageScore")
            .and_then(Value::as_f64)
            .map(|score| (score / 20.0) as f32),
        language: Some("en".to_string()),
        content_rating: Some(if is_adult { "adult" } else { "safe" }.to_string()),
        status: parse_status(media.get("status").and_then(Value::as_str)),
        initialized,
        ..CatalogItem::default()
    })
}

fn title(title: &Value, request: &Value) -> String {
    let romaji = title
        .get("romaji")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let english = title
        .get("english")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let native = title
        .get("native")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match pref(request, "preferred_title", "romaji") {
        "english" => first_non_empty([english, romaji, native]),
        "native" => first_non_empty([native, romaji, english]),
        _ => first_non_empty([romaji, english, native]),
    }
    .unwrap_or("AniList")
    .to_string()
}

fn first_non_empty(values: [&str; 3]) -> Option<&str> {
    values.into_iter().find(|value| !value.trim().is_empty())
}

fn cover(media: &Value) -> Option<String> {
    ["extraLarge", "large", "medium"]
        .into_iter()
        .find_map(|key| {
            media
                .pointer(&format!("/coverImage/{key}"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

fn description(media: &Value) -> Option<String> {
    let mut out = media
        .get("description")
        .and_then(Value::as_str)
        .map(clean_description)
        .unwrap_or_default();
    let season = media.get("season").and_then(Value::as_str);
    let season_year = media.get("seasonYear").and_then(Value::as_u64);
    if season.is_some() || season_year.is_some() {
        push_line(
            &mut out,
            &format!(
                "Release: {} {}",
                season.unwrap_or_default(),
                season_year.map(|year| year.to_string()).unwrap_or_default()
            ),
        );
    }
    if let Some(format) = media.get("format").and_then(Value::as_str) {
        push_line(&mut out, &format!("Type: {format}"));
    }
    if let Some(episodes) = media.get("episodes").and_then(Value::as_u64) {
        push_line(&mut out, &format!("Total Episode Count: {episodes}"));
    }
    (!out.trim().is_empty()).then(|| out.trim().to_string())
}

fn clean_description(value: &str) -> String {
    let normalized = value
        .replace("<br>\n", "\n")
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n");
    html::strip_tags(&normalized)
}

fn push_line(out: &mut String, line: &str) {
    if !out.trim().is_empty() {
        out.push('\n');
    }
    out.push_str(line.trim());
}

fn main_studio(media: &Value) -> Option<String> {
    let edges = media.pointer("/studios/edges")?.as_array()?;
    edges
        .iter()
        .find(|edge| edge.get("isMain").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| edges.first())
        .and_then(|edge| edge.pointer("/node/name").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn episodes_from_mal(
    anilist_id: u64,
    mal_id: u64,
    episode_count: u64,
    request: &Value,
) -> Vec<VideoEpisode> {
    let mut episodes = Vec::new();
    let mark_fillers = pref_bool(request, "preferred_mark_fillers", true);
    let mut page = 1;
    loop {
        let body = client()
            .get(format!("{JIKAN_URL}/anime/{mal_id}/episodes?page={page}"))
            .header("Accept", "application/json")
            .referer(BASE_URL)
            .send_text()
            .ok();
        let Some(body) = body else {
            return Vec::new();
        };
        let root: Value = serde_json::from_str(&body).unwrap_or_default();
        let data = root
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if data.is_empty()
            && root
                .pointer("/pagination/last_visible_page")
                .and_then(Value::as_u64)
                == Some(1)
        {
            return single_episode_from_mal(anilist_id, mal_id);
        }
        for episode in data {
            let Some(number) = episode.get("mal_id").and_then(Value::as_u64) else {
                continue;
            };
            let title = episode
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let filler = mark_fillers
                && episode
                    .get("filler")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            episodes.push(VideoEpisode {
                key: episode_key(anilist_id, number),
                title: Some(match title {
                    Some(title) if title != format!("Episode {number}") => {
                        format!("Ep. {number} - {title}")
                    }
                    _ => format!("Episode {number}"),
                }),
                episode_number: Some(number as f32),
                date_uploaded: episode
                    .get("aired")
                    .and_then(Value::as_str)
                    .and_then(parse_jikan_date),
                url: Some(anime_url(anilist_id)),
                language: Some("en".to_string()),
                is_filler: filler,
                labels: if filler {
                    vec!["Filler episode".to_string()]
                } else {
                    Vec::new()
                },
                ..VideoEpisode::default()
            });
        }
        if !root
            .pointer("/pagination/has_next_page")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        page += 1;
    }
    for number in (episodes.len() as u64 + 1)..=episode_count {
        episodes.push(simple_episode(anilist_id, number));
    }
    episodes.retain(|episode| {
        episode_count == 0 || episode.episode_number.unwrap_or_default() <= episode_count as f32
    });
    episodes.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    episodes
}

fn single_episode_from_mal(anilist_id: u64, mal_id: u64) -> Vec<VideoEpisode> {
    let date = client()
        .get(format!("{JIKAN_URL}/anime/{mal_id}"))
        .header("Accept", "application/json")
        .referer(BASE_URL)
        .send_text()
        .ok()
        .and_then(|body| {
            let root: Value = serde_json::from_str(&body).ok()?;
            root.pointer("/data/aired/from")
                .and_then(Value::as_str)
                .and_then(parse_jikan_date)
        });
    vec![VideoEpisode {
        key: episode_key(anilist_id, 1),
        title: Some("Episode 1".to_string()),
        episode_number: Some(1.0),
        date_uploaded: date,
        url: Some(anime_url(anilist_id)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }]
}

fn fallback_episodes(id: u64, episode_count: u64) -> Vec<VideoEpisode> {
    (1..=episode_count)
        .map(|number| simple_episode(id, number))
        .rev()
        .collect()
}

fn simple_episode(id: u64, number: u64) -> VideoEpisode {
    VideoEpisode {
        key: episode_key(id, number),
        title: Some(format!("Episode {number}")),
        episode_number: Some(number as f32),
        url: Some(anime_url(id)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn episode_key(id: u64, number: u64) -> String {
    json!({ "id": id, "episode": number }).to_string()
}

fn parse_jikan_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(0..10)?)
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value {
        Some("FINISHED") => ItemStatus::Completed,
        Some("RELEASING") | Some("NOT_YET_RELEASED") => ItemStatus::Ongoing,
        Some("CANCELLED") => ItemStatus::Cancelled,
        Some("HIATUS") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn anime_url(id: u64) -> String {
    format!("{BASE_URL}/anime/{id}")
}

fn id_from_url(input: &str) -> Option<String> {
    input
        .split("/anime/")
        .nth(1)
        .map(|value| {
            value
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(value)
                .to_string()
        })
        .filter(|value| value.parse::<u64>().is_ok())
}

fn id_from_key(key: &str) -> u64 {
    if let Ok(value) = serde_json::from_str::<Value>(key) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            return id;
        }
    }
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
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
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

fn string_array_filter(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

const SORT_QUERY: &str = r#"query($page:Int,$perPage:Int,$isAdult:Boolean,$type:MediaType,$sort:[MediaSort],$status:MediaStatus,$search:String,$genres:[String],$year:String,$seasonYear:Int,$season:MediaSeason,$format:[MediaFormat],$countryOfOrigin:CountryCode){Page(page:$page,perPage:$perPage){pageInfo{hasNextPage}media(isAdult:$isAdult,type:$type,sort:$sort,status:$status,search:$search,genre_in:$genres,startDate_like:$year,seasonYear:$seasonYear,season:$season,format_in:$format,countryOfOrigin:$countryOfOrigin){id title{romaji english native}coverImage{extraLarge large medium}isAdult averageScore}}}"#;
const DETAILS_QUERY: &str = r#"query($id:Int,$type:MediaType){Media(id:$id,type:$type){id title{romaji english native}coverImage{extraLarge large medium}bannerImage description season seasonYear format status(version:2)genres episodes isAdult averageScore studios{edges{isMain node{name}}}}}"#;
const EPISODE_QUERY: &str = r#"query($id:Int,$type:MediaType){Media(id:$id,type:$type){id idMal status episodes nextAiringEpisode{episode}}}"#;
const PAGE_FIXTURE: &str = r#"{"data":{"Page":{"pageInfo":{"hasNextPage":false},"media":[{"id":1,"title":{"romaji":"Sample AniList","english":"Sample AniList","native":"Sample AniList"},"coverImage":{"extraLarge":"https://fixtures.invalid/anilist-cover.jpg","large":null,"medium":null},"isAdult":false,"averageScore":80}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"Media":{"id":1,"title":{"romaji":"Sample AniList","english":"Sample AniList","native":"Sample AniList"},"coverImage":{"extraLarge":"https://fixtures.invalid/anilist-cover.jpg","large":null,"medium":null},"bannerImage":null,"description":"Fixture details.","season":"SPRING","seasonYear":2024,"format":"TV","status":"FINISHED","genres":["Action"],"episodes":1,"isAdult":false,"averageScore":80,"studios":{"edges":[]}}}}"#;
const EPISODE_FIXTURE: &str = r#"{"data":{"Media":{"id":1,"idMal":null,"status":"FINISHED","episodes":1,"nextAiringEpisode":null}}}"#;

export_video_source!(SOURCE);
