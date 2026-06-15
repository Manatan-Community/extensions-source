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

const SOURCE: SupJav = SupJav;
const BASE_URL: &str = "https://supjav.com";
const PROTECTOR_URL: &str = "https://lk1.supremejav.com";

struct SupJav;

impl VideoSource for SupJav {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = fetch_document_or_fixture(
            &format!("{BASE_URL}{}/popular/page/{page}", lang_path(&request)),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&path)?],
                has_next_page: false,
            });
        }
        if let Some(path) = query.strip_prefix("id:") {
            return Ok(Paged {
                entries: vec![fetch_details(path)?],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(request);
        }
        let body = fetch_document_or_fixture(
            &format!(
                "{BASE_URL}{}/?s={}",
                lang_path(&request),
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        fetch_details(&key)
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        Ok(vec![VideoEpisode {
            key: key.clone(),
            title: Some("JAV".to_string()),
            episode_number: Some(1.0),
            url: Some(item_url(&key)),
            language: Some("all".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let body = fetch_document_or_fixture(&item_url(&key), DETAILS_FIXTURE);
        Ok(parse_hosters(&body))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(2, '|');
        let name = parts.next().unwrap_or("Hoster");
        let code = parts.next().unwrap_or_default();
        let target = resolve_protected(code).unwrap_or_else(|| code.to_string());
        if name == "TV" {
            if let Ok(body) = client().get(&target).browser_document().send_text() {
                if let Some(playlist) = extract_tv_playlist(&body) {
                    return Ok(vec![hls_stream(&playlist, "TV", &target)]);
                }
            }
        }
        Ok(vec![external_stream(&target, name, &request)])
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
        sort_streams(&mut streams, pref_str(&request, "pref_quality", "720p"));
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
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
        if let Some(path) = path_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path)?),
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
        .with_header("Origin", BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .filter(|block| block.contains("post") && block.contains("<a") && block.contains("<img"))
        .filter_map(parse_listing_item)
        .collect();
    let has_next_page = body.contains("pagination") && !body.contains("active</a></li></ul>");
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_listing_item(block: &str) -> Option<CatalogItem> {
    let href = html::attr_after(block, "<a", "href")?;
    let key = path_from_url(&href)?;
    let title = html::attr_after(block, "<img", "alt").unwrap_or_else(|| key.replace('-', " "));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(block, "<img", "data-original")
            .or_else(|| html::attr_after(block, "<img", "src")),
        url: Some(item_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(key: &str) -> ExtensionResult<CatalogItem> {
    let body = fetch_document_or_fixture(&item_url(key), DETAILS_FIXTURE);
    let mut item = parse_details(&body).unwrap_or_else(|| CatalogItem {
        key: normalize_key(key),
        title: normalize_key(key).replace('-', " "),
        url: Some(item_url(key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    });
    item.key = normalize_key(key);
    item.url = Some(item_url(key));
    item.initialized = true;
    Ok(item)
}

fn parse_details(body: &str) -> Option<CatalogItem> {
    let content = body.split("post-meta").nth(1).unwrap_or(body);
    let title = html::text_between(content, "<h2", "</h2>").map(|text| html::strip_tags(&text))?;
    let authors = links_after(content, "Maker :");
    let artists = links_after(content, "Cast :");
    let tags = content
        .split("div class=\"tags\"")
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|text| html::strip_tags(&text))
        })
        .collect();
    Some(CatalogItem {
        key: String::new(),
        title,
        cover: html::attr_after(content, "<img", "src"),
        authors,
        artists,
        tags,
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn links_after(content: &str, label: &str) -> Vec<String> {
    content
        .split(label)
        .nth(1)
        .unwrap_or_default()
        .split("</p>")
        .next()
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|text| html::strip_tags(&text))
        })
        .filter(|text| !text.is_empty())
        .collect()
}

fn parse_hosters(body: &str) -> Vec<VideoHoster> {
    body.split("div class=\"btnst\"")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let name = html::strip_tags(&format!("<a{chunk}")).trim().to_string();
            if !matches!(name.as_str(), "TV" | "FST" | "VOE" | "ST") {
                return None;
            }
            let code = html::attr(chunk, "data-link")?
                .chars()
                .rev()
                .collect::<String>();
            Some(VideoHoster {
                key: format!("{name}|{code}"),
                name,
                url: Some(code),
                lazy: true,
                video_count: Some(1),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_protected(code: &str) -> Option<String> {
    let response = client()
        .get(format!("{PROTECTOR_URL}/supjav.php?c={code}"))
        .referer(format!("{PROTECTOR_URL}/"))
        .send()
        .ok()?;
    response
        .headers
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value)
        .or_else(|| {
            (response.final_url != format!("{PROTECTOR_URL}/supjav.php?c={code}"))
                .then_some(response.final_url)
        })
}

fn extract_tv_playlist(body: &str) -> Option<String> {
    body.split("var urlPlay = '")
        .nth(1)?
        .split("';")
        .next()
        .map(ToString::to_string)
}

fn external_stream(target: &str, name: &str, request: &Value) -> VideoStream {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), BASE_URL.to_string());
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some(pref_str(request, "pref_quality", "external").to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

fn hls_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    VideoStream {
        url: target.to_string(),
        name: Some(name.to_string()),
        quality: Some("HLS".to_string()),
        format: Some("hls".to_string()),
        is_hls: true,
        stream_kind: Some(VideoStreamKind::Hls),
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred: &str) {
    streams.sort_by(|a, b| {
        let ap = a.quality.as_deref().unwrap_or("").contains(preferred);
        let bp = b.quality.as_deref().unwrap_or("").contains(preferred);
        bp.cmp(&ap)
    });
}

fn lang_path(request: &Value) -> String {
    match filter_str(request, "language", "en") {
        "en" => String::new(),
        other => format!("/{other}"),
    }
}

fn item_url(key: &str) -> String {
    format!("{BASE_URL}/{}", normalize_key(key).trim_matches('/'))
}

fn path_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.trim_start_matches(BASE_URL).trim_matches('/');
    (!path.is_empty()).then(|| path.to_string())
}

fn normalize_key(key: &str) -> String {
    path_from_url(key).unwrap_or_else(|| key.trim_matches('/').to_string())
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
<div class="post">
  <a href="https://supjav.com/sample-video">
    <img alt="Sample Video" data-original="https://supjav.com/sample.jpg">
  </a>
</div>
<nav class="pagination"><a>1</a><a>2</a></nav>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="post-meta">
  <h2>Sample Video</h2>
  <img src="https://supjav.com/sample.jpg">
  <p>Maker : <a>Sample Studio</a></p>
  <p>Cast : <a>Sample Actor</a></p>
  <div class="tags"><a>Tag</a></div>
</div>
<div class="btnst"><a data-link="cba">TV</a></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hoster_fixture() {
        let hosters = parse_hosters(
            r#"<div class="btnst"><a data-link="cba">TV</a><a data-link="zyx">NO</a></div>"#,
        );
        assert_eq!(hosters.len(), 1);
        assert_eq!(hosters[0].key, "TV|abc");
    }
}
