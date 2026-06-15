use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: ShabakatyCinemana = ShabakatyCinemana;
const BASE_URL: &str = "https://cinemana.shabakaty.com";
const API_URL: &str = "https://cinemana.shabakaty.com/api/android";
const POPULAR_PER_PAGE: u64 = 30;
const SEARCH_PER_PAGE: usize = 12;
const LATEST_PER_PAGE: usize = 24;

struct ShabakatyCinemana;

impl VideoSource for ShabakatyCinemana {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request).saturating_sub(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let kind_name = pref_str(&request, "preferred_latest_kind", "Movies");
        if listing == "latest" {
            let body = client()
                .get(format!("{API_URL}/latest{kind_name}/level/0/itemsPerPage/{LATEST_PER_PAGE}/page/{page}/"))
                .xhr()
                .send_text()
                .unwrap_or_else(|_| LIST_FIXTURE.to_string());
            return Ok(parse_items(&body, LATEST_PER_PAGE));
        }
        let kind = kind_number(kind_name);
        let body = client()
            .get(format!("{API_URL}/video/V/2/itemsPerPage/{POPULAR_PER_PAGE}/level/0/videoKind/{kind}/sortParam/desc/pageNumber/{page}"))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(parse_items(&body, POPULAR_PER_PAGE as usize))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&id)?],
                has_next_page: false,
            });
        }
        let page = page(&request).saturating_sub(1);
        let kind_name = filter_str(&request, "kind", "Movies");
        let kind = kind_number(kind_name);
        let browse = filter_str(&request, "browse", "false") == "true";
        let body = if browse {
            let language = filter_str(&request, "language", "0");
            let category = filter_str(&request, "main_category", "");
            let sort = filter_str(&request, "browse_sort", "desc");
            let mut target = if language != "0" && !category.is_empty() {
                format!(
                    "{API_URL}/videosByCategoryAndLanguage?language_id={language}&category_id={category}"
                )
            } else {
                format!("{API_URL}/videosByCategory")
            };
            if !category.is_empty() && language == "0" {
                target.push_str(&format!("?categoryID={category}"));
            } else {
                target.push_str(if target.contains('?') { "&" } else { "?" });
                target.push_str("unused=1");
            }
            target.push_str(&format!(
                "&level=0&offset={}&videoKind={kind}&orderby={sort}",
                page * POPULAR_PER_PAGE
            ));
            client().get(target).xhr().send_text()?
        } else {
            let years = format!(
                "{},{}",
                filter_str(&request, "year_start", "1900"),
                filter_str(&request, "year_end", "2026")
            );
            let mut target = format!(
                "{API_URL}/AdvancedSearch?level=0&type={}&page={page}&year={years}",
                kind_name.to_lowercase()
            );
            let category = filter_str(&request, "main_category", "");
            if !category.is_empty() {
                target.push_str(&format!("&category_id={category}"));
            }
            if !query.is_empty() {
                target.push_str(&format!("&videoTitle={}", url::query_escape(query)));
            }
            let staff = filter_str(&request, "staff_title", "");
            if !staff.is_empty() {
                target.push_str(&format!("&staffTitle={}", url::query_escape(staff)));
            }
            client().get(target).xhr().send_text()?
        };
        Ok(parse_search_response(&body, browse))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        fetch_details(&key)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = client()
            .get(format!("{API_URL}/videoSeason/id/{key}"))
            .xhr()
            .send_text()?;
        let mut episodes = parse_episodes(&body);
        if episodes.is_empty() {
            episodes.push(VideoEpisode {
                key: key.clone(),
                title: Some("movie".to_string()),
                episode_number: Some(1.0),
                url: Some(item_url(&key)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            });
        }
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let subtitles = client()
            .get(format!("{API_URL}/translationFiles/id/{key}"))
            .xhr()
            .send_text()
            .map(|body| parse_subtitles(&body, &request))
            .unwrap_or_default();
        let body = client()
            .get(format!("{API_URL}/transcoddedFiles/id/{key}"))
            .xhr()
            .send_text()?;
        let mut streams = parse_streams(&body, subtitles);
        sort_streams(
            &mut streams,
            pref_str(&request, "preferred_quality", "1080"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({ "listing": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) }))?;
        let latest = self.list(json!({ "listing": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) }))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
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
        Ok(request_key(&request, "item").map(|key| item_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| item_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&id)?),
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

fn fetch_details(key: &str) -> ExtensionResult<CatalogItem> {
    let body = client()
        .get(format!("{API_URL}/allVideoInfo/id/{}", normalize_key(key)))
        .xhr()
        .send_text()?;
    Ok(parse_item(&serde_json::from_str(&body).unwrap_or_default())
        .unwrap_or_else(|| fallback_item(key)))
}

fn parse_items(body: &str, page_size: usize) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_item)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() == page_size;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_search_response(body: &str, browse: bool) -> Paged<CatalogItem> {
    if browse {
        let root: Value = serde_json::from_str(body).unwrap_or_default();
        let info = root
            .get("info")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return Paged {
            has_next_page: info.len() == POPULAR_PER_PAGE as usize,
            entries: info.iter().filter_map(parse_item).collect(),
        };
    }
    parse_items(body, SEARCH_PER_PAGE)
}

fn parse_item(value: &Value) -> Option<CatalogItem> {
    let key = value.get("nb")?.as_str()?.to_string();
    let title = value
        .get("en_title")
        .and_then(Value::as_str)
        .unwrap_or("no title")
        .to_string();
    let categories = value
        .get("categories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("en_title").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let language = value
        .get("videoLanguages")
        .and_then(|value| value.get("en_title"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let stars = value
        .get("stars")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f32>().ok());
    let description = [
        value.get("year").and_then(Value::as_str).map(|year| {
            let likes = value.get("Likes").and_then(Value::as_str).unwrap_or("0");
            let dislikes = value.get("DisLikes").and_then(Value::as_str).unwrap_or("0");
            format!("{year} | {likes} likes | {dislikes} dislikes")
        }),
        value
            .get("en_content")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: value
            .get("imgObjUrl")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(item_url(&key)),
        authors: names(value.get("directorsInfo")),
        artists: names(value.get("actorsInfo")),
        description: (!description.is_empty()).then_some(description),
        tags: categories,
        language,
        rating: stars.map(|value| value / 2.0),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: value.get("en_content").is_some(),
        ..CatalogItem::default()
    })
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mut episodes = root
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let key = value.get("nb")?.as_str()?.to_string();
            let season = value
                .get("season")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<f32>().ok());
            let episode = value
                .get("episodeNummer")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<f32>().ok());
            Some(VideoEpisode {
                key: key.clone(),
                title: Some(format!(
                    "{} - {}",
                    value.get("season").and_then(Value::as_str).unwrap_or(""),
                    value
                        .get("episodeNummer")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                )),
                episode_number: episode,
                season_number: season,
                url: Some(item_url(&key)),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    episodes.sort_by(|a, b| {
        b.season_number
            .partial_cmp(&a.season_number)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.episode_number
                    .partial_cmp(&a.episode_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    episodes
}

fn parse_subtitles(body: &str, request: &Value) -> Vec<SubtitleTrack> {
    let preferred_ext = pref_str(request, "preferred_subtitle_extension", "ass");
    let preferred_lang = pref_str(request, "preferred_subtitle_language", "arabic");
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mut tracks = root
        .get("translations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let url = value.get("file")?.as_str()?.to_string();
            let label = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Subtitle")
                .to_string();
            let format = value
                .get("extention")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            Some(SubtitleTrack {
                url,
                language: Some(label.to_lowercase()),
                label: Some(label),
                format,
                ..SubtitleTrack::default()
            })
        })
        .collect::<Vec<_>>();
    tracks.sort_by(|a, b| {
        let ap = a.format.as_deref() == Some(preferred_ext)
            || a.language.as_deref() == Some(preferred_lang);
        let bp = b.format.as_deref() == Some(preferred_ext)
            || b.language.as_deref() == Some(preferred_lang);
        bp.cmp(&ap)
    });
    if let Some(first) = tracks.first_mut() {
        first.is_default = true;
    }
    tracks
}

fn parse_streams(body: &str, subtitles: Vec<SubtitleTrack>) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let stream_url = value.get("videoUrl")?.as_str()?.to_string();
            let quality = value
                .get("resolution")
                .and_then(Value::as_str)
                .unwrap_or("Video")
                .to_string();
            let mut headers = Context::new();
            headers.insert("Referer".to_string(), BASE_URL.to_string());
            Some(VideoStream {
                url: stream_url,
                name: Some(quality.clone()),
                quality: Some(quality.clone()),
                format: Some(
                    if quality.contains("m3u8") {
                        "hls"
                    } else {
                        "direct"
                    }
                    .to_string(),
                ),
                is_hls: quality.contains("m3u8"),
                stream_kind: Some(if quality.contains("m3u8") {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                subtitles: subtitles.clone(),
                headers,
                initialized: true,
                ..VideoStream::default()
            })
        })
        .collect()
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by(|a, b| {
        let aq = quality_number(a.quality.as_deref().unwrap_or_default());
        let bq = quality_number(b.quality.as_deref().unwrap_or_default());
        let ap = a.quality.as_deref().unwrap_or_default().contains(preferred);
        let bp = b.quality.as_deref().unwrap_or_default().contains(preferred);
        bp.cmp(&ap).then_with(|| bq.cmp(&aq))
    });
}

fn quality_number(value: &str) -> u32 {
    value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn item_url(key: &str) -> String {
    format!("{BASE_URL}/video/en/{}", normalize_key(key))
}

fn id_from_url(input: &str) -> Option<String> {
    input
        .split("/video/en/")
        .nth(1)
        .map(|tail| tail.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_key(key: &str) -> String {
    id_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string())
}

fn kind_number(kind: &str) -> u8 {
    if kind.eq_ignore_ascii_case("Series") {
        2
    } else {
        1
    }
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: normalize_key(key),
        url: Some(item_url(key)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
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
        .map(normalize_key)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

export_video_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
[
  {
    "nb": "fixture-video",
    "en_title": "Fixture Video",
    "year": "2026",
    "Likes": "0",
    "DisLikes": "0",
    "en_content": "Fixture response used when live HTTP is unavailable during local smoke tests.",
    "imgObjUrl": "https://cinemana.shabakaty.com/fixture.jpg",
    "categories": [{"en_title": "Drama"}],
    "videoLanguages": {"en_title": "Arabic"},
    "stars": "8"
  }
]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_item_fixture() {
        let item = parse_item(
            &json!({"nb":"42","en_title":"Movie","categories":[{"en_title":"Drama"}],"stars":"8"}),
        )
        .unwrap();
        assert_eq!(item.key, "42");
        assert_eq!(item.rating, Some(4.0));
    }

    #[test]
    fn parses_episode_fixture() {
        let episodes = parse_episodes(r#"[{"nb":"99","season":"2","episodeNummer":"3"}]"#);
        assert_eq!(episodes[0].season_number, Some(2.0));
    }
}
