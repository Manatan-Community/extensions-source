use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: Myanime = Myanime;
const BASE_URL: &str = "https://myanime.live";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";

struct Myanime;

impl VideoSource for Myanime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{BASE_URL}/page/{page}/")
        } else {
            format!("{BASE_URL}/category/donghua-list/page/{page}/")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(parse_listing(&body, listing(&request) == "latest"))
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
        let page = page(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/page/{page}/?s={}",
                url::query_escape(&query.replace(' ', "+"))
            )
        } else if let Some(subpage) =
            filter_value(&request, "subpage").filter(|value| !value.is_empty())
        {
            format!("{BASE_URL}/{}/page/{page}/", subpage.trim_matches('/'))
        } else {
            format!("{BASE_URL}/category/donghua-list/page/{page}/")
        };
        let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
        Ok(parse_listing(&body, true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/playlist-sample/".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/playlist-sample/".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episode_list(&body, &absolute_url(&path), 0))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/sample-episode/".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, WATCH_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &episode_url, &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let server = parts.next().unwrap_or("External");
        let target = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        if target.is_empty() {
            return Ok(Vec::new());
        }
        let mut streams = vec![external_stream(target, server, referer)];
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
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Donghua List".to_string(),
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
        Ok(request_key(&request, "item").map(|path| absolute_url(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute_url(&path)))
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
        .with_header("User-Agent", UA)
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, latest_style: bool) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "main#main > article.post")
            .filter_map(|article| parse_card(article, latest_style))
            .collect(),
        has_next_page: select_all(&doc, "script")
            .any(|script| script.inner_html().contains("infiniteScroll")),
    }
}

fn parse_card(article: ElementRef<'_>, latest_style: bool) -> Option<CatalogItem> {
    let href = attr(&article, "h2.entry-header-title > a", "href")
        .or_else(|| attr(&article, "a", "href"))?;
    let mut title = text(&article, "h2.entry-header-title > a")
        .or_else(|| text(&article, "a"))
        .unwrap_or_else(|| title_from_path(&href));
    if latest_style {
        title = strip_episode_suffix(&title);
    } else if let Some(rest) = title.strip_prefix("Playlist ") {
        title = rest.to_string();
    }
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: image_url(article),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let target = absolute_url(path);
    let body = get_or_fixture(&target, DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text(&doc, "h1.entry-title")
            .or_else(|| select_text(&doc, "h2.entry-header-title > a"))
            .or_else(|| select_text(&doc, "title"))
            .map(|value| strip_episode_suffix(&value))
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(&doc, "div.entry-content img", "src")
            .or_else(|| select_attr(&doc, "article.post img", "src"))
            .map(|value| absolute_url(&value)),
        description: select_text(&doc, "div.entry-content").filter(|value| !value.is_empty()),
        tags: select_all(&doc, "span > a[href*=/tag/], a[rel=tag]")
            .map(|link| collect_text(&link))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(target),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episode_list(body: &str, page_url: &str, depth: usize) -> Vec<VideoEpisode> {
    if depth > 4 {
        return Vec::new();
    }
    let doc = Html::parse_document(body);
    let path = path_key(page_url);
    let item_name = path
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .unwrap_or_default();

    if item_name.starts_with("playlist-") {
        let episodes = select_all(&doc, "div.dpt-wrapper > div.dpt-entry")
            .filter_map(|entry| {
                let link = select_all_in(entry, "a.dpt-permalink").next()?;
                episode_from_anchor(link)
            })
            .collect::<Vec<_>>();
        if !episodes.is_empty() {
            return episodes;
        }
    }

    if let Some(all_url) = select_all(&doc, "a[href]")
        .find(|link| collect_text(link).contains("All Episodes"))
        .and_then(|link| attr(&link, "", "href"))
    {
        let target = absolute_url(&all_url);
        let next_body = get_or_fixture(&target, DETAILS_FIXTURE, page_url);
        return parse_episode_list(&next_body, &target, depth + 1);
    }

    if path.trim_start_matches('/').starts_with("tag/") {
        let mut episodes = Vec::new();
        for page in 1..=20 {
            let target = format!("{}/page/{page}/", page_url.trim_end_matches('/'));
            let page_body = get_or_fixture(
                &target,
                if page == 1 { DETAILS_FIXTURE } else { "" },
                page_url,
            );
            if page_body.is_empty() {
                break;
            }
            let page_doc = Html::parse_document(&page_body);
            episodes.extend(
                select_all(&page_doc, "main#main > article.post")
                    .filter_map(|article| {
                        select_all_in(article, "h2.entry-header-title > a").next()
                    })
                    .filter_map(episode_from_anchor),
            );
            let has_next = select_all(&page_doc, "script")
                .any(|script| script.inner_html().contains("infiniteScroll"));
            if !has_next {
                break;
            }
        }
        if !episodes.is_empty() {
            return episodes;
        }
    }

    if select_all(
        &doc,
        "iframe.youtube-player[src], div.entry-content iframe[src]",
    )
    .next()
    .is_some()
    {
        return vec![VideoEpisode {
            key: path_key(page_url),
            title: select_text(&doc, "title").or_else(|| select_text(&doc, "h1.entry-title")),
            episode_number: Some(episode_number_from_text(
                &select_text(&doc, "title").unwrap_or_default(),
            )),
            url: Some(absolute_url(page_url)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        }];
    }

    if let Some(tag_url) = select_attr(&doc, "span > a[href*=/tag/]", "href") {
        let target = absolute_url(&tag_url);
        let next_body = get_or_fixture(&target, DETAILS_FIXTURE, page_url);
        return parse_episode_list(&next_body, &target, depth + 1);
    }

    Vec::new()
}

fn episode_from_anchor(link: ElementRef<'_>) -> Option<VideoEpisode> {
    let href = attr(&link, "", "href")?;
    let name = collect_text(&link);
    Some(VideoEpisode {
        key: path_key(&href),
        title: Some(name.clone()),
        episode_number: Some(episode_number_from_text(&name)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    })
}

fn parse_hosters(body: &str, episode_url: &str, _request: &Value) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    select_all(&doc, "div.entry-content iframe[src]")
        .filter_map(|frame| attr(&frame, "", "src"))
        .map(|src| normalize_embed(&src))
        .filter(|src| supported_embed(src))
        .map(|src| {
            let name = hoster_name(&src);
            VideoHoster {
                key: format!("{name}|{src}|{episode_url}"),
                name: name.to_string(),
                url: Some(src),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(episode_url),
                ..VideoHoster::default()
            }
        })
        .collect()
}

fn normalize_embed(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        absolute_url(input)
    }
}

fn supported_embed(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("dailymotion")
        || lower.contains("ok.ru")
        || lower.contains("okru")
        || lower.contains("youtube.com")
        || lower.contains("youtu.be")
        || lower.contains("gdriveplayer")
}

fn hoster_key(input: &str) -> &'static str {
    let lower = input.to_ascii_lowercase();
    if lower.contains("youtube") || lower.contains("youtu.be") {
        "youtube"
    } else if lower.contains("dailymotion") {
        "dailymotion"
    } else if lower.contains("ok.ru") || lower.contains("okru") {
        "okru"
    } else if lower.contains("gdriveplayer") {
        "gdriveplayer"
    } else {
        "external"
    }
}

fn hoster_name(input: &str) -> &'static str {
    match hoster_key(input) {
        "youtube" => "YouTube",
        "dailymotion" => "Dailymotion",
        "okru" => "Ok.ru",
        "gdriveplayer" => "Gdriveplayer",
        _ => "External",
    }
}

fn external_stream(target: &str, server: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(server.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_quality = request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080");
    let preferred_server = preferred_server(request);
    streams.sort_by_key(|stream| {
        let name = stream
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let quality = stream.quality.as_deref().unwrap_or_default();
        (
            quality.contains(preferred_quality),
            name.contains(preferred_server),
            quality_score(quality),
        )
    });
    streams.reverse();
    for stream in streams {
        let name = stream
            .name
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let quality = stream.quality.as_deref().unwrap_or_default();
        stream.preferred = quality.contains(preferred_quality) || name.contains(preferred_server);
    }
}

fn quality_score(quality: &str) -> i32 {
    quality
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

fn preferred_server(request: &Value) -> &str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_server"))
        .or_else(|| request.get("preferred_server"))
        .and_then(Value::as_str)
        .unwrap_or("dailymotion")
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if let Some(obj) = next.as_object_mut() {
        obj.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    next
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter_value<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .or_else(|| value.get("url"))
                .or(Some(value))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(path_key)
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn path_from_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(BASE_URL) {
        Some(path_key(trimmed))
    } else if trimmed.starts_with('/') {
        Some(path_key(trimmed))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if let Some(rest) = input.strip_prefix(BASE_URL) {
        return path_key(rest);
    }
    if let Some(rest) = input.strip_prefix("https://www.myanime.live") {
        return path_key(rest);
    }
    let without_fragment = input.split('#').next().unwrap_or(input);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    format!("/{}", without_query.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("Myanime")
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

fn strip_episode_suffix(input: &str) -> String {
    input
        .split(" Episode")
        .next()
        .unwrap_or(input)
        .split(" episode")
        .next()
        .unwrap_or(input)
        .trim()
        .to_string()
}

fn episode_number_from_text(input: &str) -> f32 {
    input
        .split("pisode ")
        .nth(1)
        .and_then(|part| part.split_whitespace().next())
        .and_then(|part| part.parse::<f32>().ok())
        .unwrap_or(0.0)
}

fn image_url(article: ElementRef<'_>) -> Option<String> {
    attr(&article, "img[src]", "src")
        .or_else(|| attr(&article, "img[data-src]", "data-src"))
        .map(|image| absolute_url(&image))
}

fn selector(query: &str) -> Selector {
    Selector::parse(query).unwrap()
}

fn select_all<'a>(doc: &'a Html, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    doc.select(&selector(query)).collect::<Vec<_>>().into_iter()
}

fn select_all_in<'a>(element: ElementRef<'a>, query: &str) -> impl Iterator<Item = ElementRef<'a>> {
    element
        .select(&selector(query))
        .collect::<Vec<_>>()
        .into_iter()
}

fn select_attr(doc: &Html, query: &str, name: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .and_then(|element| attr(&element, "", name))
}

fn attr(element: &ElementRef<'_>, query: &str, name: &str) -> Option<String> {
    let target = if query.is_empty() {
        *element
    } else {
        select_all_in(*element, query).next()?
    };
    target
        .value()
        .attr(name)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn select_text(doc: &Html, query: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn text(element: &ElementRef<'_>, query: &str) -> Option<String> {
    select_all_in(*element, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(
        &element
            .text()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

const LIST_FIXTURE: &str = r#"
<main id="main">
  <article class="post">
    <h2 class="entry-header-title"><a href="https://myanime.live/playlist-sample/">Playlist Sample Donghua</a></h2>
    <img src="https://myanime.live/sample.jpg">
  </article>
</main>
<script>var infiniteScroll = true;</script>
"#;

const SEARCH_FIXTURE: &str = r#"
<main id="main">
  <article class="post">
    <h2 class="entry-header-title"><a href="https://myanime.live/sample-episode/">Sample Donghua Episode 1</a></h2>
    <img src="https://myanime.live/sample.jpg">
  </article>
</main>
"#;

const DETAILS_FIXTURE: &str = r#"
<html>
<head><title>Sample Donghua Episode 1</title></head>
<body>
  <h1 class="entry-title">Sample Donghua</h1>
  <div class="entry-content">
    <p>Sample description.</p>
    <iframe class="youtube-player" src="https://www.youtube.com/embed/sample"></iframe>
  </div>
  <div class="dpt-wrapper">
    <div class="dpt-entry"><a class="dpt-permalink" href="https://myanime.live/sample-episode/">Episode 1</a></div>
  </div>
</body>
</html>
"#;

const WATCH_FIXTURE: &str = r#"
<div class="entry-content">
  <iframe src="https://www.dailymotion.com/embed/video/sample"></iframe>
  <iframe src="https://ok.ru/videoembed/sample"></iframe>
  <iframe class="youtube-player" src="https://www.youtube.com/embed/sample"></iframe>
</div>
"#;

export_video_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_fixture() {
        let page = parse_listing(LIST_FIXTURE, false);
        assert_eq!(page.entries[0].key, "/playlist-sample/");
        assert_eq!(page.entries[0].title, "Sample Donghua");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_playlist_episodes() {
        let episodes =
            parse_episode_list(DETAILS_FIXTURE, "https://myanime.live/playlist-sample/", 0);
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].episode_number, Some(1.0));
    }

    #[test]
    fn parses_supported_hosters() {
        let hosters = parse_hosters(
            WATCH_FIXTURE,
            "https://myanime.live/sample-episode/",
            &json!({}),
        );
        assert_eq!(hosters.len(), 3);
        assert_eq!(hosters[0].name, "Dailymotion");
    }
}
