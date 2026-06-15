use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: RouVideo = RouVideo;
const BASE_URL: &str = "https://rou.video";
const API_URL: &str = "https://rou.video/api";

struct RouVideo;

impl VideoSource for RouVideo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let order = if listing == "latest" {
            "createdAt"
        } else {
            "likeCount"
        };
        let body = doc_client()
            .get(format!(
                "{BASE_URL}/v?order={order}&page={}",
                page(&request)
            ))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_video_list(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = query
            .strip_prefix("v:")
            .map(ToString::to_string)
            .or_else(|| id_from_url(query))
        {
            return Ok(Paged {
                entries: vec![fetch_details(&id)],
                has_next_page: false,
            });
        }
        if let Some(tag) = query.strip_prefix("t:") {
            let body = doc_client()
                .get(format!(
                    "{BASE_URL}/t/{}?page={}",
                    url::query_escape(tag),
                    page(&request)
                ))
                .browser_document()
                .send_text()
                .unwrap_or_default();
            return Ok(parse_video_list(&body));
        }
        let category = filter(&request, "category").unwrap_or_default();
        if query.is_empty() || category == "featured" {
            match category.as_str() {
                "watching" => {
                    let body = api_client()
                        .get(format!("{API_URL}/v/watching"))
                        .send_text()
                        .unwrap_or_default();
                    return Ok(parse_api_videos(&body));
                }
                "featured" => {
                    let body = doc_client()
                        .get(format!("{BASE_URL}/home"))
                        .browser_document()
                        .send_text()
                        .unwrap_or_default();
                    return Ok(parse_hot_video_list(
                        &body,
                        filter(&request, "sort").as_deref(),
                    ));
                }
                value if !value.is_empty() && value != "all-videos" => {
                    let body = doc_client()
                        .get(format!(
                            "{BASE_URL}/t/{}?order={}&page={}",
                            url::query_escape(value),
                            filter(&request, "sort").unwrap_or_else(|| "createdAt".to_string()),
                            page(&request)
                        ))
                        .browser_document()
                        .send_text()
                        .unwrap_or_default();
                    return Ok(parse_video_list(&body));
                }
                _ => {}
            }
        }
        let tag = filter(&request, "tag").unwrap_or_default();
        let hot = filter(&request, "hot_search").unwrap_or_default();
        let actual_query = if query.is_empty() {
            hot.as_str()
        } else {
            query
        };
        let target = if !actual_query.is_empty() {
            let mut out = format!(
                "{BASE_URL}/search?q={}&page={}",
                url::query_escape(actual_query),
                page(&request)
            );
            let category_part = if !category.is_empty() && category != "all-videos" {
                &category
            } else {
                &tag
            };
            if !category_part.is_empty() {
                out.push_str("&t=");
                out.push_str(&url::query_escape(category_part));
            }
            out
        } else if !tag.is_empty() {
            format!(
                "{BASE_URL}/t/{}?page={}",
                url::query_escape(&tag),
                page(&request)
            )
        } else {
            format!(
                "{BASE_URL}/v?order={}&page={}",
                filter(&request, "sort").unwrap_or_else(|| "createdAt".to_string()),
                page(&request)
            )
        };
        let body = doc_client()
            .get(target)
            .browser_document()
            .send_text()
            .unwrap_or_default();
        Ok(parse_video_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = normalize_id(&request_key(&request, "item").unwrap_or_default());
        let body = doc_client()
            .get(item_url(&key))
            .browser_document()
            .send_text()
            .unwrap_or_default();
        let video = parse_video_details(&body);
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some(key.clone()),
            episode_number: Some(1.0),
            date_uploaded: None,
            thumbnail: video
                .as_ref()
                .and_then(|video| video.cover_image_url.clone()),
            duration_seconds: video.as_ref().and_then(|video| video.duration),
            url: Some(item_url(&key)),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = normalize_id(&request_key(&request, "episode").unwrap_or_default());
        let body = api_client()
            .get(format!("{API_URL}/v/{key}"))
            .send_text()
            .unwrap_or_default();
        let Ok(data) = serde_json::from_str::<VideoData>(&body) else {
            return Ok(Vec::new());
        };
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), format!("{BASE_URL}/"));
        Ok(vec![VideoStream {
            url: data.video.video_url,
            name: Some("Default".to_string()),
            quality: Some("auto".to_string()),
            format: Some("hls".to_string()),
            is_hls: true,
            stream_kind: Some(VideoStreamKind::Hls),
            headers,
            preferred: true,
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
                title: "Most Liked".to_string(),
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
        Ok(request_key(&request, "item").map(|key| item_url(&normalize_id(&key))))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&normalize_id(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id)),
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

fn doc_client() -> HttpClient {
    HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_client() -> HttpClient {
    HttpClient::browser()
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Accept", "application/json, text/plain, */*")
        .with_cookies_for(BASE_URL)
}

fn parse_video_list(body: &str) -> Paged<CatalogItem> {
    let Some(data) = next_data(body) else {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    };
    let payload = serde_json::from_str::<VideoList>(&data).unwrap_or_default();
    Paged {
        entries: payload
            .props
            .page_props
            .videos
            .into_iter()
            .map(video_to_item)
            .collect(),
        has_next_page: payload.props.page_props.page_num < payload.props.page_props.total_page,
    }
}

fn parse_hot_video_list(body: &str, sort: Option<&str>) -> Paged<CatalogItem> {
    let Some(data) = next_data(body) else {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    };
    let payload = serde_json::from_str::<HotVideoList>(&data).unwrap_or_default();
    let mut videos = payload.props.page_props.all_videos();
    match sort {
        Some("viewCount") => videos.sort_by_key(|video| std::cmp::Reverse(video.view_count)),
        Some("likeCount") => {
            videos.sort_by_key(|video| std::cmp::Reverse(video.like_count.unwrap_or_default()))
        }
        _ => videos.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
    }
    videos.dedup_by(|a, b| a.id == b.id);
    Paged {
        entries: videos.into_iter().map(video_to_item).collect(),
        has_next_page: false,
    }
}

fn parse_api_videos(body: &str) -> Paged<CatalogItem> {
    let videos = serde_json::from_str::<Vec<Video>>(body).unwrap_or_default();
    Paged {
        entries: videos.into_iter().map(video_to_item).collect(),
        has_next_page: false,
    }
}

fn fetch_details(key: &str) -> CatalogItem {
    let key = normalize_id(key);
    let body = doc_client()
        .get(item_url(&key))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    parse_video_details(&body)
        .map(video_to_item)
        .unwrap_or_else(|| CatalogItem {
            key: key.clone(),
            title: key.clone(),
            url: Some(item_url(&key)),
            language: Some("all".to_string()),
            content_rating: Some("adult".to_string()),
            status: ItemStatus::Unknown,
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_video_details(body: &str) -> Option<Video> {
    let data = next_data(body)?;
    serde_json::from_str::<VideoDetails>(&data)
        .ok()?
        .props
        .page_props
        .video
}

fn video_to_item(video: Video) -> CatalogItem {
    let mut description = String::new();
    if let Some(resolution) = video
        .sources
        .as_ref()
        .and_then(|sources| sources.first())
        .and_then(|source| source.resolution)
    {
        description.push_str(&format!("Resolution: {resolution}p\n"));
    }
    if let Some(duration) = video.duration {
        description.push_str(&format!("Duration: {}\n", format_duration(duration as u64)));
    }
    description.push_str(&format!("View: {}", video.view_count));
    if let Some(likes) = video.like_count {
        description.push_str(&format!(" - Like: {likes}"));
    }
    if let Some(reference) = &video.reference {
        description.push_str(&format!("\nRef: {reference}"));
    }
    if let Some(text) = &video.description {
        description.push_str("\n\n");
        description.push_str(text);
    }
    let major = video.tags.first().cloned();
    CatalogItem {
        key: video.id.clone(),
        title: video.name.clone(),
        cover: video.cover_image_url.clone(),
        url: Some(item_url(&video.id)),
        description: Some(description).filter(|value| !value.trim().is_empty()),
        authors: major.iter().cloned().collect(),
        artists: major.into_iter().collect(),
        tags: video.code.into_iter().chain(video.tags).collect(),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn next_data(body: &str) -> Option<String> {
    let script = body.split("id=\"__NEXT_DATA__\"").nth(1)?;
    html::text_between(script, ">", "</script>")
}

fn id_from_url(input: &str) -> Option<String> {
    if input.trim().is_empty() {
        return None;
    }
    if input.starts_with("http") && !input.contains("rou.video") {
        return None;
    }
    input
        .split("/v/")
        .nth(1)
        .or_else(|| input.strip_prefix("v:"))
        .or_else(|| input.rsplit('/').next())
        .map(|value| {
            value
                .split('?')
                .next()
                .unwrap_or(value)
                .trim_matches('/')
                .to_string()
        })
        .filter(|value| !value.is_empty() && value != "rou.video" && value != "v")
}

fn normalize_id(input: &str) -> String {
    id_from_url(input).unwrap_or_else(|| input.trim_matches('/').to_string())
}

fn item_url(key: &str) -> String {
    format!("{BASE_URL}/v/{}", key.trim_matches('/'))
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours}:{minutes}:{seconds}")
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

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoList {
    #[serde(default)]
    props: VideoListProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoListProps {
    #[serde(default)]
    page_props: VideoListPageProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoListPageProps {
    #[serde(default)]
    videos: Vec<Video>,
    #[serde(default)]
    page_num: i64,
    #[serde(default)]
    total_page: i64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotVideoList {
    #[serde(default)]
    props: HotVideoListProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotVideoListProps {
    #[serde(default)]
    page_props: HotVideoListPageProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotVideoListPageProps {
    #[serde(default)]
    latest_videos: Vec<Video>,
    #[serde(default)]
    daily_hot_cnav: Vec<Video>,
    #[serde(default)]
    daily_hot_selfie: Vec<Video>,
    #[serde(default)]
    daily_hot91: Vec<Video>,
    #[serde(default)]
    daily_only_fans: Vec<Video>,
    #[serde(default)]
    daily_jv: Vec<Video>,
    #[serde(default)]
    hot_cnav: Vec<Video>,
    #[serde(default)]
    hot_selfie: Vec<Video>,
    #[serde(default)]
    hot91: Vec<Video>,
}

impl HotVideoListPageProps {
    fn all_videos(self) -> Vec<Video> {
        [
            self.latest_videos,
            self.daily_hot_cnav,
            self.daily_hot_selfie,
            self.daily_hot91,
            self.daily_only_fans,
            self.daily_jv,
            self.hot_cnav,
            self.hot_selfie,
            self.hot91,
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetails {
    #[serde(default)]
    props: VideoDetailsProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetailsProps {
    #[serde(default)]
    page_props: VideoDetailsPageProps,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetailsPageProps {
    video: Option<Video>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Video {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "vid")]
    code: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    view_count: i64,
    #[serde(default)]
    like_count: Option<i64>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    cover_image_url: Option<String>,
    #[serde(default)]
    sources: Option<Vec<Source>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    #[serde(default)]
    resolution: Option<i64>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoData {
    video: VideoObject,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoObject {
    video_url: String,
}

export_video_source!(SOURCE);
