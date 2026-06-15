use std::collections::HashSet;

use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: Rule34Video = Rule34Video;
const BASE_URL: &str = "https://rule34video.com";

struct Rule34Video;

impl VideoSource for Rule34Video {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = listing_url(&request);
        let body = fetch_or_fixture(&target, LIST_FIXTURE, BASE_URL);
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
                entries: vec![fetch_details(&path)],
                has_next_page: false,
            });
        }
        if let Some(slug) = query.strip_prefix("slug:") {
            return Ok(Paged {
                entries: vec![fetch_details(&format!(
                    "/search/{}",
                    slug.trim_matches('/')
                ))],
                has_next_page: false,
            });
        }

        let target = search_url(&request, query);
        let body = fetch_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/videos/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/videos/sample/".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        Ok(vec![VideoEpisode {
            key: path_key(&key),
            title: parse_title(&body, &key).or_else(|| Some("Video".to_string())),
            episode_number: Some(1.0),
            thumbnail: parse_cover(&body, BASE_URL),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        }])
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/videos/sample/".to_string());
        let page_url = absolute_url(&key);
        let body = fetch_or_fixture(&page_url, DETAILS_FIXTURE, BASE_URL);
        let mut streams = parse_streams(&body, &page_url, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Latest Updates".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| absolute_url(&key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| absolute_url(&key)))
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

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_origin(BASE_URL)
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

fn listing_url(request: &Value) -> String {
    if pref_bool(request, "uploader_filter_enabled", false) {
        let uploader_id = pref_str(request, "uploader_id", "").trim().to_string();
        if !uploader_id.is_empty() {
            return format!(
                "{BASE_URL}/members/{}/videos/?mode=async&function=get_block&block_id=list_videos_uploaded_videos&sort_by=&from_videos={}",
                url::query_escape(&uploader_id),
                page(request)
            );
        }
    }
    format!("{BASE_URL}/latest-updates/{}/", page(request))
}

fn search_url(request: &Value, query: &str) -> String {
    let order = filter_str(request, "order", "latest-updates");
    let sort_by = match order {
        "most-popular" => "video_viewed",
        "top-rated" => "rating",
        _ => "post_date",
    };
    let category = selected_value(filter_str(request, "category", ""));
    let tag_ids = tag_ids(request);
    let page = page(request);
    if query.is_empty() {
        format!(
            "{BASE_URL}/search/?flag1={}&sort_by={sort_by}&from_videos={page}&tag_ids=all%2C{}",
            url::query_escape(&category),
            url::query_escape(&tag_ids)
        )
    } else {
        let slug = query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .trim_matches('/')
            .to_string();
        format!(
            "{BASE_URL}/search/{}/?flag1={}&sort_by={sort_by}&from_videos={page}&tag_ids=all%2C{}",
            url::query_escape(&slug),
            url::query_escape(&category),
            url::query_escape(&tag_ids)
        )
    }
}

fn tag_ids(request: &Value) -> String {
    let explicit = filter_str(request, "tag_ids", "").trim().to_string();
    if !explicit.is_empty() {
        return explicit;
    }
    let tag = filter_str(request, "tag", "").trim().to_string();
    if tag.is_empty() {
        return String::new();
    }
    let target = format!("{BASE_URL}/search_ajax.php?tag={}", url::query_escape(&tag));
    let body = fetch_or_fixture(&target, TAG_FIXTURE, BASE_URL);
    parse_tag_ids(&body)
}

fn parse_tag_ids(body: &str) -> String {
    let doc = Html::parse_document(body);
    select_all(&doc, "div.item input")
        .filter_map(|input| input.value().attr("value"))
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let entries = select_all(&doc, "div.item.thumb, .item.thumb")
        .filter_map(card_item)
        .filter(|item| seen.insert(item.key.clone()))
        .collect();
    Paged {
        entries,
        has_next_page: select_all(&doc, "div.item.pager.next a, a.next, link[rel=\"next\"]")
            .next()
            .is_some(),
    }
}

fn card_item(element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&element, "a.th, a[href]", "href")?;
    let key = path_key(&href);
    let title = text(&element, "a.th div.thumb_title, div.thumb_title")
        .or_else(|| attr(&element, "img", "alt"))
        .unwrap_or_else(|| title_from_path(&key));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: attr(&element, "a.th div.img img, img", "data-original")
            .or_else(|| attr(&element, "a.th div.img img, img", "data-src"))
            .or_else(|| attr(&element, "a.th div.img img, img", "src"))
            .map(|value| absolute_remote(&value, BASE_URL)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = parse_title(body, path)?;
    let rows = select_all(&doc, "div.row")
        .map(|row| collect_text(&row))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let info = select_all(&doc, "div.info.row div.item_info span")
        .map(|item| collect_text(&item))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let mut description = Vec::new();
    for line in select_all(&doc, "div.label > em")
        .map(|item| html::html_unescape(&item.inner_html().replace("<br>", "\n")))
        .filter(|value| !value.trim().is_empty())
    {
        description.push(line.trim().to_string());
    }
    if let Some(uploaded) = info.first() {
        description.push(format!("Uploaded: {uploaded}"));
    }
    for row in &rows {
        if row.contains("Artist") || row.contains("Categories") || row.contains("Uploaded by") {
            description.push(row.clone());
        }
    }
    if let Some(views) = info.get(1) {
        description.push(format!(
            "Views: {}",
            views.split_whitespace().next().unwrap_or(views)
        ));
    }
    if let Some(duration) = info.get(2) {
        description.push(format!("Duration: {duration}"));
    }

    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: parse_cover(body, BASE_URL),
        authors: select_all(&doc, "a.item span.name")
            .map(|element| collect_text(&element))
            .filter(|value| !value.is_empty())
            .collect(),
        tags: select_all(&doc, "div.row_spacer a.tag_item, a.tag_item")
            .map(|element| collect_text(&element))
            .filter(|value| !value.is_empty() && !value.contains("Suggest"))
            .collect(),
        description: (!description.is_empty()).then(|| description.join("\n")),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_title(body: &str, path: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    select_text(&doc, "h1.title_video")
        .or_else(|| select_text(&doc, "h1"))
        .or_else(|| meta(&doc, "meta[property=\"og:title\"]"))
        .or_else(|| Some(title_from_path(path)))
}

fn parse_cover(body: &str, base: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    select_attr(&doc, "meta[property=\"og:image\"]", "content")
        .or_else(|| select_attr(&doc, "video", "poster"))
        .or_else(|| select_attr(&doc, "img[data-original]", "data-original"))
        .or_else(|| select_attr(&doc, "img[data-src]", "data-src"))
        .map(|value| absolute_remote(&value, base))
}

fn parse_streams(body: &str, page_url: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    let mut seen = HashSet::new();
    let mut streams = Vec::new();

    for url in media_candidates(body) {
        if seen.insert(url.clone()) {
            streams.push(media_stream(
                &url,
                quality_from_text_or_url("", &url),
                page_url,
                request,
            ));
        }
    }

    for element in select_all(
        &doc,
        "a.tag_item[href], a[href*=\"download\"], a[href*=\"get_file\"]",
    ) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let label = collect_text(&element);
        let candidate = absolute_remote(href, page_url);
        if !looks_like_download(&candidate, &label) {
            continue;
        }
        let stream_url = if is_media_url(&candidate) {
            candidate
        } else {
            resolve_download_url(&candidate, page_url).unwrap_or(candidate)
        };
        if seen.insert(stream_url.clone()) {
            streams.push(media_stream(
                &stream_url,
                quality_from_text_or_url(&label, &stream_url),
                page_url,
                request,
            ));
        }
    }
    streams
}

fn resolve_download_url(target: &str, referer: &str) -> Option<String> {
    let response = client(referer)
        .get(target)
        .referer(referer)
        .header(
            "Accept",
            "video/webm,video/ogg,video/*;q=0.9,application/ogg;q=0.7,audio/*;q=0.6,*/*;q=0.5",
        )
        .send()
        .ok()?;
    response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            (response.final_url != target && !response.final_url.trim().is_empty())
                .then_some(response.final_url)
        })
}

fn media_candidates(body: &str) -> Vec<String> {
    let cleaned = html::html_unescape(&body.replace("\\/", "/").replace("\\\"", "\""));
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for part in cleaned.split(['"', '\'', '<', '>', ' ', '\n', '\r', '\t', '(', ')', ',']) {
        let Some(start) = part.find("http") else {
            continue;
        };
        let mut candidate = part[start..].trim_matches([';', ']']).to_string();
        if let Some(end) = media_end(&candidate) {
            candidate.truncate(end);
        }
        if is_media_url(&candidate) && seen.insert(candidate.clone()) {
            urls.push(candidate);
        }
    }
    urls
}

fn media_end(value: &str) -> Option<usize> {
    [".mp4", ".webm", ".m3u8", ".mpd"]
        .into_iter()
        .find_map(|needle| {
            value
                .find(needle)
                .map(|index| index + needle.len())
                .map(|end| {
                    value[end..]
                        .find(['&', '?'])
                        .map(|suffix| end + suffix)
                        .unwrap_or(end)
                })
        })
}

fn media_stream(target: &str, quality: String, referer: &str, request: &Value) -> VideoStream {
    let lower = target.to_ascii_lowercase();
    let is_hls = lower.contains(".m3u8");
    let is_dash = lower.contains(".mpd");
    VideoStream {
        url: target.to_string(),
        name: Some("Rule34Video".to_string()),
        quality: Some(quality.clone()),
        format: Some(
            if is_hls {
                "hls"
            } else if is_dash {
                "dash"
            } else if lower.contains(".webm") {
                "webm"
            } else {
                "mp4"
            }
            .to_string(),
        ),
        is_hls,
        is_dash,
        preferred: quality == preferred_quality(request),
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else if is_dash {
            VideoStreamKind::Dash
        } else {
            VideoStreamKind::Direct
        }),
        headers: stream_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn looks_like_download(url: &str, label: &str) -> bool {
    let lower_url = url.to_ascii_lowercase();
    let lower_label = label.to_ascii_lowercase();
    is_media_url(url)
        || lower_url.contains("download")
        || lower_url.contains("get_file")
        || ["2160p", "1080p", "720p", "480p", "360p"]
            .into_iter()
            .any(|quality| lower_label.contains(quality))
}

fn is_media_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains(".mp4")
        || lower.contains(".webm")
        || lower.contains(".m3u8")
        || lower.contains(".mpd")
}

fn quality_from_text_or_url(label: &str, url: &str) -> String {
    for value in [label, url] {
        for quality in ["2160p", "1080p", "720p", "480p", "360p", "240p"] {
            if value.to_ascii_lowercase().contains(quality) {
                return quality.to_string();
            }
        }
    }
    label
        .split_whitespace()
        .find(|part| !part.is_empty())
        .unwrap_or("Video")
        .to_string()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| stream.quality.as_deref() != Some(preferred.as_str()));
}

fn stream_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers.insert("Accept-Language".to_string(), "en-US,en;q=0.5".to_string());
    headers
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|selector| doc.select(&selector).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn select_text(doc: &Html, selector: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .and_then(|element| element.value().attr(name).map(ToString::to_string))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    element
        .select(&selector)
        .next()
        .and_then(|value| value.value().attr(name).map(ToString::to_string))
}

fn meta(doc: &Html, selector: &str) -> Option<String> {
    select_attr(doc, selector, "content")
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("/video") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_origin
        .split('#')
        .next()
        .unwrap_or(without_origin)
        .split('?')
        .next()
        .unwrap_or(without_origin);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{BASE_URL}{}", path_key(path))
    }
}

fn absolute_remote(path: &str, base: &str) -> String {
    if path.starts_with("//") {
        format!("https:{path}")
    } else if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        url::join_url(base, path)
    }
}

fn title_from_path(path: &str) -> String {
    path_key(path)
        .trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("rule34video")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn pref_str<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
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

fn preferred_quality(request: &Value) -> String {
    pref_str(request, "preferred_quality", "1080p").to_string()
}

fn selected_value(value: &str) -> String {
    value
        .rsplit_once(':')
        .map(|(_, part)| part.to_string())
        .unwrap_or_else(|| value.to_string())
}

const LIST_FIXTURE: &str = r#"
<div class="item thumb">
  <a class="th" href="/videos/sample-video/">
    <div class="img"><img data-original="https://img.example/sample.jpg"></div>
    <div class="thumb_title">Sample Video</div>
  </a>
</div>
<div class="item pager next"><a href="/latest-updates/2/">Next</a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="item thumb">
  <a class="th" href="/videos/search-sample/">
    <div class="img"><img src="https://img.example/search.jpg" alt="Search Sample"></div>
    <div class="thumb_title">Search Sample</div>
  </a>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:image" content="https://img.example/detail.jpg">
<h1 class="title_video">Sample Video</h1>
<div class="info row">
  <div class="item_info"><span>Jan 01, 2024</span></div>
  <div class="item_info"><span>1,234 views</span></div>
  <div class="item_info"><span>10:00</span></div>
</div>
<div class="row"><div class="label"><em>Fixture description.<br>Second line.</em></div></div>
<div class="row"><div class="col"><div class="label">Artist</div><a class="item"><span class="name">Sample Artist</span></a></div></div>
<div class="row_spacer"><a class="tag_item">tag one</a></div>
<div class="row"><div class="label">Download</div><a class="tag_item" href="https://media.example/video-720p.mp4">Download 720p</a></div>
"#;

const TAG_FIXTURE: &str = r#"
<div class="item"><input value="123"><label>sample tag</label></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries.len(), 1);
        assert!(listing.has_next_page);
        assert_eq!(listing.entries[0].title, "Sample Video");
    }

    #[test]
    fn parses_details_fixture() {
        let item = parse_details(DETAILS_FIXTURE, "/videos/sample-video/").unwrap();
        assert_eq!(item.title, "Sample Video");
        assert_eq!(item.authors, vec!["Sample Artist"]);
        assert!(item.description.unwrap().contains("Uploaded: Jan 01, 2024"));
    }

    #[test]
    fn parses_stream_fixture() {
        let streams = parse_streams(DETAILS_FIXTURE, BASE_URL, &Value::Null);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].quality.as_deref(), Some("720p"));
    }

    #[test]
    fn parses_tag_ids_fixture() {
        assert_eq!(parse_tag_ids(TAG_FIXTURE), "123");
    }
}

export_video_source!(SOURCE);
