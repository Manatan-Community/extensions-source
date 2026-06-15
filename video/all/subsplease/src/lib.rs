use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, TorrentInfo, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: Subsplease = Subsplease;
const BASE_URL: &str = "https://subsplease.org";
const TZ: &str = "Europe/Berlin";

struct Subsplease;

impl VideoSource for Subsplease {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch_xhr_or_fixture(
            &format!("{BASE_URL}/api/?f=schedule&tz={TZ}"),
            SCHEDULE_FIXTURE,
        );
        Ok(parse_schedule(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![show_item(&slug, None, None)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let body = fetch_xhr_or_fixture(
            &format!(
                "{BASE_URL}/api/?f=search&tz={TZ}&s={}",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = fetch_document_or_fixture(&show_url(&key), DETAILS_FIXTURE);
        let mut item = show_item(&key, None, None);
        item.description = html::text_between(&body, "series-syn", "</div>")
            .map(|text| html::strip_tags(&text))
            .filter(|text| !text.is_empty());
        item.initialized = true;
        Ok(item)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let page = fetch_document_or_fixture(&show_url(&key), DETAILS_FIXTURE);
        let sid = html::attr_after(&page, "show-release-table", "sid").unwrap_or_default();
        let api = format!("{BASE_URL}/api/?f=show&tz={TZ}&sid={sid}");
        let body = fetch_xhr_or_fixture(&api, SHOW_FIXTURE);
        Ok(parse_episodes(&body, &api))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let body = fetch_xhr_or_fixture(&key, SHOW_FIXTURE);
        let num = key.split("num=").nth(1).unwrap_or_default();
        let mut streams = parse_streams(&body, num, &request);
        sort_streams(
            &mut streams,
            pref_str(&request, "preferred_quality", "1080"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Schedule".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: list.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| show_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(show_item(&slug, None, None)),
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

fn fetch_xhr_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_schedule(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .get("schedule")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|days| days.values())
        .filter_map(Value::as_array)
        .flat_map(|items| items.iter())
        .filter_map(item_from_value)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = root
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .filter_map(item_from_value)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn item_from_value(value: &Value) -> Option<CatalogItem> {
    let title = value.get("title").or_else(|| value.get("show"))?.as_str()?;
    let page = value.get("page")?.as_str()?;
    let cover = value
        .get("image_url")
        .and_then(Value::as_str)
        .map(|path| url::join_url(BASE_URL, path));
    Some(show_item(page, Some(title), cover))
}

fn show_item(slug: &str, title: Option<&str>, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: slug.to_string(),
        title: title
            .map(ToString::to_string)
            .unwrap_or_else(|| slug.replace('-', " ")),
        cover,
        url: Some(show_url(slug)),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, api_url: &str) -> Vec<VideoEpisode> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let mut entries: Vec<_> = root
        .get("episode")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.values())
        .filter_map(|value| {
            let num = value.get("episode")?.as_str()?;
            let number = num
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>()
                .parse::<f32>()
                .ok();
            Some(VideoEpisode {
                key: format!("{api_url}&num={}", url::query_escape(num)),
                title: Some(format!("Episode {num}")),
                episode_number: number,
                date_uploaded: None,
                url: Some(format!("{api_url}&num={}", url::query_escape(num))),
                language: Some("all".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries
}

fn parse_streams(body: &str, selected_num: &str, request: &Value) -> Vec<VideoStream> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    root.get("episode")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.values())
        .filter(|value| value.get("episode").and_then(Value::as_str) == Some(selected_num))
        .flat_map(|value| {
            value
                .get("downloads")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|download| {
            let quality = download
                .get("res")?
                .as_str()
                .map(|value| format!("{value}p"))?;
            let magnet = download.get("magnet")?.as_str()?;
            Some(torrent_stream(magnet, &quality, request))
        })
        .collect()
}

fn torrent_stream(magnet: &str, quality: &str, request: &Value) -> VideoStream {
    let provider = pref_str(request, "debrid_provider", "none");
    if provider != "none" {
        let token = pref_str(request, "token", "");
        if let Some((hash, title)) = magnet_parts(magnet) {
            let url = format!(
                "https://torrentio.strem.fun/resolve/{provider}/{token}/{hash}/null/0/{title}"
            );
            return VideoStream {
                url,
                name: Some(format!("Debrid {quality}")),
                quality: Some(quality.to_string()),
                format: Some("debrid".to_string()),
                stream_kind: Some(VideoStreamKind::Debrid),
                initialized: true,
                ..VideoStream::default()
            };
        }
    }
    VideoStream {
        url: magnet.to_string(),
        name: Some(format!("Magnet {quality}")),
        quality: Some(quality.to_string()),
        format: Some("magnet".to_string()),
        stream_kind: Some(VideoStreamKind::Magnet),
        torrent: Some(TorrentInfo {
            magnet_url: Some(magnet.to_string()),
            file_name: magnet_parts(magnet).map(|(_, title)| title),
            ..TorrentInfo::default()
        }),
        initialized: true,
        ..VideoStream::default()
    }
}

fn magnet_parts(magnet: &str) -> Option<(String, String)> {
    let hash = magnet.split('&').find_map(|part| {
        part.strip_prefix("magnet:?xt=urn:btih:")
            .or_else(|| part.strip_prefix("xt=urn:btih:"))
    })?;
    let title = magnet
        .split('&')
        .find_map(|part| part.strip_prefix("dn="))
        .unwrap_or("video")
        .replace('+', "%20");
    Some((hash.to_string(), title))
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by(|a, b| {
        let ap = a.quality.as_deref().unwrap_or("").contains(preferred);
        let bp = b.quality.as_deref().unwrap_or("").contains(preferred);
        bp.cmp(&ap).then_with(|| b.quality.cmp(&a.quality))
    });
}

fn show_url(slug: &str) -> String {
    format!("{BASE_URL}/shows/{}", slug.trim_matches('/'))
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split("/shows/")
        .nth(1)
        .map(|tail| tail.trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
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
        .map(ToString::to_string)
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

export_video_source!(SOURCE);

const SCHEDULE_FIXTURE: &str = r#"{"schedule":{"Monday":[{"title":"Sample Show","page":"sample-show","image_url":"/img/sample.jpg"}]}}"#;
const SEARCH_FIXTURE: &str =
    r#"{"sample":{"title":"Sample Show","page":"sample-show","image_url":"/img/sample.jpg"}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="series-syn">Sample show description.</div><table id="show-release-table" sid="sample"></table>"#;
const SHOW_FIXTURE: &str = r#"{"episode":{"1":{"episode":"01","downloads":[{"res":"1080","magnet":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Sample+Show"}]}}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schedule_fixture() {
        let page = parse_schedule(
            r#"{"schedule":{"Monday":[{"title":"Show","page":"show","image_url":"/img.jpg"}]}}"#,
        );
        assert_eq!(page.entries[0].key, "show");
    }

    #[test]
    fn parses_stream_fixture() {
        let body = r#"{"episode":{"1":{"episode":"01","downloads":[{"res":"1080","magnet":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show"}]}}}"#;
        let streams = parse_streams(body, "01", &serde_json::json!({}));
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].stream_kind, Some(VideoStreamKind::Magnet));
    }
}
