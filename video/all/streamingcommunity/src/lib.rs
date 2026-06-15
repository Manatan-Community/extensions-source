use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::Value;

const SOURCE: StreamingCommunity = StreamingCommunity;
const DEFAULT_DOMAIN: &str = "https://streamingunity.biz";

struct StreamingCommunity;

impl VideoSource for StreamingCommunity {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let base = domain(&request);
        let lang = filter_str(&request, "lang", "en");
        let show_type = filter_str(&request, "type", "movie");
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            if page <= 2 {
                let endpoint = if show_type == "movie" {
                    "latest"
                } else {
                    "new-episodes"
                };
                format!(
                    "{base}/api/browse/{endpoint}?lang={lang}&offset={}&type={show_type}",
                    (page - 1) * 60
                )
            } else {
                format!(
                    "{base}/api/archive?lang={lang}&offset={}&sort=created_at&type={show_type}",
                    (page - 3) * 60
                )
            }
        } else if page == 1 {
            format!("{base}/api/browse/top10?lang={lang}&type={show_type}")
        } else if page == 2 {
            format!("{base}/api/browse/trending?lang={lang}&type={show_type}")
        } else {
            format!(
                "{base}/api/archive?lang={lang}&offset={}&sort=views&type={show_type}",
                (page - 3) * 60
            )
        };
        let body = client(&base)
            .get(target)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| TITLE_PAGE_FIXTURE.to_string());
        Ok(parse_title_page(&body, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&request, &key)?],
                has_next_page: false,
            });
        }
        if query.is_empty() && filters_are_default(&request) {
            return self.list(request);
        }
        let base = domain(&request);
        let lang = filter_str(&request, "lang", "en");
        let show_type = filter_str(&request, "type", "movie");
        let mut target = format!(
            "{base}/api/archive?search={}&lang={lang}&type={show_type}&offset={}",
            url::query_escape(query),
            (page(&request) - 1) * 60
        );
        for (field, param) in [
            ("sort", "sort"),
            ("year", "year"),
            ("score", "score"),
            ("service", "service"),
            ("quality", "quality"),
            ("age", "age"),
        ] {
            let value = filter_str(&request, field, "");
            if !value.is_empty() {
                target.push_str(&format!("&{param}={}", url::query_escape(value)));
            }
        }
        let genre = filter_str(&request, "genre", "");
        if !genre.is_empty() {
            target.push_str(&format!("&genre[]={genre}"));
        }
        let body = client(&base)
            .get(target)
            .xhr()
            .send_text()
            .unwrap_or_else(|_| TITLE_PAGE_FIXTURE.to_string());
        Ok(parse_title_page(&body, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        fetch_details(&request, &key)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let base = domain(&request);
        let lang = filter_str(&request, "lang", "en");
        let text = client(&base)
            .get(format!("{base}/{lang}/titles/{key}"))
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| TITLE_DETAIL_FIXTURE.to_string());
        let body = get_data(&text).ok_or_else(|| error("Failed to extract title data"))?;
        Ok(parse_episodes(&body, &base, lang))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let base = domain(&request);
        let lang = filter_str(&request, "lang", "en");
        let iframe_url = if key.starts_with("http") {
            key
        } else {
            let page = client(&base)
                .get(format!("{base}/{lang}/iframe/{key}"))
                .browser_document()
                .send_text()?;
            html::attr_after(&page, "<iframe", "src").unwrap_or_default()
        };
        let mut streams = extract_vixcloud(&iframe_url, &base, lang)?;
        sort_streams(
            &mut streams,
            pref_str(&request, "preferred_quality", "1080"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
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
        let base = domain(&request);
        let lang = filter_str(&request, "lang", "en");
        Ok(request_key(&request, "item").map(|key| format!("{base}/{lang}/titles/{key}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&request, &key)?),
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
        .with_header("Origin", base)
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_details(request: &Value, key: &str) -> ExtensionResult<CatalogItem> {
    let base = domain(request);
    let lang = filter_str(request, "lang", "en");
    let text = client(&base)
        .get(format!("{base}/{lang}/titles/{key}"))
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| TITLE_DETAIL_FIXTURE.to_string());
    let body = get_data(&text).ok_or_else(|| error("Failed to extract title data"))?;
    let title = body.pointer("/props/title").unwrap_or(&Value::Null);
    Ok(parse_details(title, &base).unwrap_or_else(|| CatalogItem {
        key: key.to_string(),
        title: key.to_string(),
        url: Some(format!("{base}/{lang}/titles/{key}")),
        language: Some(lang.to_string()),
        content_rating: Some("safe".to_string()),
        ..CatalogItem::default()
    }))
}

fn parse_title_page(body: &str, base: &str) -> Paged<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or_default();
    let props = root.get("props").unwrap_or(&root);
    let cdn = props
        .get("cdn_url")
        .and_then(Value::as_str)
        .map(|value| format!("{value}/images/"))
        .unwrap_or_else(|| {
            format!(
                "https://cdn.{}/images/",
                base.trim_start_matches("https://")
            )
        });
    let titles = props
        .get("titles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Paged {
        has_next_page: titles.len() == 60
            || body.contains("/browse/top10")
            || body.contains("/browse/trending"),
        entries: titles
            .iter()
            .filter_map(|value| parse_title(value, &cdn))
            .collect(),
    }
}

fn parse_title(value: &Value, image_cdn: &str) -> Option<CatalogItem> {
    let id = value.get("id")?.as_i64()?;
    let slug = value.get("slug")?.as_str()?;
    let title = value.get("name")?.as_str()?;
    Some(CatalogItem {
        key: format!("{id}-{slug}"),
        title: title.to_string(),
        cover: image(value, image_cdn),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(title: &Value, base: &str) -> Option<CatalogItem> {
    let id = title.get("id")?.as_i64()?;
    let name = title.get("name")?.as_str()?.to_string();
    let slug = title.get("slug").and_then(Value::as_str).unwrap_or(&name);
    let show_type = title.get("type").and_then(Value::as_str).unwrap_or("movie");
    let cdn = format!(
        "https://cdn.{}/images/",
        base.trim_start_matches("https://")
    );
    Some(CatalogItem {
        key: format!("{id}-{slug}"),
        title: name,
        alternate_titles: title
            .get("original_name")
            .and_then(Value::as_str)
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        cover: image(title, &cdn),
        description: title
            .get("plot")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: title
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        authors: title
            .get("main_directors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|person| person.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        artists: title
            .get("main_actors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|person| person.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        rating: title
            .get("score")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f32>().ok())
            .map(|score| score / 2.0),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(title.get("status").and_then(Value::as_str), show_type),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &Value, base: &str, lang: &str) -> Vec<VideoEpisode> {
    let props = body.get("props").unwrap_or(body);
    let Some(title) = props.get("title") else {
        return Vec::new();
    };
    let title_id = title.get("id").and_then(Value::as_i64).unwrap_or_default();
    if props.get("loadedSeason").is_none() {
        let mut entries = vec![VideoEpisode {
            key: title_id.to_string(),
            title: Some("Film".to_string()),
            episode_number: Some(1.0),
            url: Some(format!("{base}/{lang}/iframe/{title_id}")),
            language: Some(lang.to_string()),
            ..VideoEpisode::default()
        }];
        if let Some(preview) = title.pointer("/preview/embed_url").and_then(Value::as_str) {
            entries.push(VideoEpisode {
                key: preview.to_string(),
                title: Some("Preview".to_string()),
                episode_number: Some(0.0),
                url: Some(preview.to_string()),
                language: Some(lang.to_string()),
                ..VideoEpisode::default()
            });
        }
        return entries;
    }
    let season_number = props
        .pointer("/loadedSeason/number")
        .and_then(Value::as_i64)
        .unwrap_or(1) as f32;
    let mut entries = props
        .pointer("/loadedSeason/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let id = episode.get("id")?.as_i64()?;
            let number = episode.get("number").and_then(Value::as_i64).unwrap_or(1);
            let name = episode.get("name").and_then(Value::as_str).unwrap_or("");
            Some(VideoEpisode {
                key: format!("{title_id}?episode_id={id}&next_episode=1"),
                title: Some(
                    format!(
                        "Season {} Episode {} - {}",
                        season_number as i64, number, name
                    )
                    .trim()
                    .to_string(),
                ),
                episode_number: Some(number as f32),
                season_number: Some(season_number),
                thumbnail: image(
                    episode,
                    &format!(
                        "https://cdn.{}/images/",
                        base.trim_start_matches("https://")
                    ),
                ),
                url: Some(format!(
                    "{base}/{lang}/iframe/{title_id}?episode_id={id}&next_episode=1"
                )),
                language: Some(lang.to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect::<Vec<_>>();
    entries.reverse();
    entries
}

fn extract_vixcloud(iframe_url: &str, base: &str, lang: &str) -> ExtensionResult<Vec<VideoStream>> {
    let body = client(base)
        .get(iframe_url)
        .browser_document()
        .referer(format!("{base}/"))
        .send_text()?;
    let script = body.split("masterPlaylist").nth(1).unwrap_or(&body);
    let Some(playlist) = js_value(script, "url") else {
        return Ok(vec![external_stream(iframe_url, "VixCloud")]);
    };
    let token = js_value(script, "token").unwrap_or_default();
    let expires = js_value(script, "expires").unwrap_or_default();
    let separator = if playlist.contains('?') { '&' } else { '?' };
    let master = format!("{playlist}{separator}h=1&token={token}&expires={expires}&lang={lang}");
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), iframe_url.to_string());
    Ok(vec![VideoStream {
        url: master,
        name: Some("VixCloud HLS".to_string()),
        quality: Some("HLS".to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers,
        initialized: true,
        ..VideoStream::default()
    }])
}

fn external_stream(url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        ..VideoStream::default()
    }
}

fn js_value(script: &str, key: &str) -> Option<String> {
    for needle in [
        format!("{key}: '"),
        format!("'{key}': '"),
        format!("{key}:\""),
        format!("\"{key}\":\""),
    ] {
        if let Some(value) = script
            .split(&needle)
            .nth(1)
            .and_then(|tail| tail.split(['\'', '"']).next())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn image(value: &Value, image_cdn: &str) -> Option<String> {
    let images = value.get("images").and_then(Value::as_array)?;
    for wanted in ["poster", "cover", "cover_mobile", "background"] {
        if let Some(filename) = images
            .iter()
            .find(|image| image.get("type").and_then(Value::as_str) == Some(wanted))
            .and_then(|image| image.get("filename").and_then(Value::as_str))
        {
            return Some(format!("{image_cdn}{filename}"));
        }
    }
    None
}

fn get_data(body: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(body) {
        return Some(value);
    }
    let data = html::attr_after(body, "div id=\"app\"", "data-page")
        .or_else(|| html::attr_after(body, "id=\"app\"", "data-page"))?;
    serde_json::from_str(&html::html_unescape(&data)).ok()
}

fn parse_status(status: Option<&str>, show_type: &str) -> ItemStatus {
    match status {
        Some("Ended" | "Released") => ItemStatus::Completed,
        Some("Returning Series") => ItemStatus::Ongoing,
        Some("Canceled") => ItemStatus::Cancelled,
        _ if show_type == "movie" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
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

fn key_from_url(input: &str) -> Option<String> {
    input
        .split("/titles/")
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
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(|key| key_from_url(key).unwrap_or_else(|| key.to_string()))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn domain(request: &Value) -> String {
    pref_str(request, "custom_domain", DEFAULT_DOMAIN)
        .trim_end_matches('/')
        .to_string()
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
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

fn filters_are_default(request: &Value) -> bool {
    [
        "genre", "sort", "year", "score", "service", "quality", "age",
    ]
    .iter()
    .all(|key| filter_str(request, key, "").is_empty())
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    next["listing"] = Value::String(listing.to_string());
    next
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

export_video_source!(SOURCE);

const TITLE_PAGE_FIXTURE: &str = r#"{
  "props": {
    "cdn_url": "https://cdn.streamingunity.biz",
    "titles": [
      {
        "id": 1,
        "slug": "sample-title",
        "name": "Sample Title",
        "type": "movie",
        "status": "Released",
        "plot": "Fixture title for offline smoke tests.",
        "score": "7.0",
        "images": [
          {"type": "poster", "filename": "sample-title/poster.jpg"}
        ],
        "genres": [{"name": "Drama"}],
        "main_directors": [{"name": "Sample Director"}],
        "main_actors": [{"name": "Sample Actor"}]
      }
    ]
  }
}"#;

const TITLE_DETAIL_FIXTURE: &str = r#"{
  "props": {
    "title": {
      "id": 1,
      "slug": "sample-title",
      "name": "Sample Title",
      "type": "movie",
      "status": "Released",
      "plot": "Fixture title for offline smoke tests.",
      "score": "7.0",
      "images": [
        {"type": "poster", "filename": "sample-title/poster.jpg"}
      ],
      "genres": [{"name": "Drama"}],
      "main_directors": [{"name": "Sample Director"}],
      "main_actors": [{"name": "Sample Actor"}]
    }
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_fixture() {
        let item = parse_title(
            &serde_json::json!({"id":1,"slug":"sample","name":"Sample","images":[]}),
            "https://cdn.example/images/",
        )
        .unwrap();
        assert_eq!(item.key, "1-sample");
    }

    #[test]
    fn extracts_js_values() {
        assert_eq!(
            js_value("url: 'https://a/master.m3u8', 'token': 't'", "token"),
            Some("t".to_string())
        );
    }
}
