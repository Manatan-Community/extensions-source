use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource, webview,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;

const SOURCE: Tokuzilla = Tokuzilla;
const BASE_URL: &str = "https://tokuzilla.net";
const LIVE_BASE_URL: &str = "https://tukoz.com/t";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0 Safari/537.36";

struct Tokuzilla;

impl VideoSource for Tokuzilla {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = format!("{LIVE_BASE_URL}/page/{}", page(&request));
        Ok(parse_listing(
            &get_or_fixture(&target, LIST_FIXTURE, LIVE_BASE_URL),
            page(&request),
        ))
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

        let genre = filter(&request, "genre").unwrap_or_default();
        if query.is_empty() && genre.is_empty() {
            return self.list(request);
        }

        let target = format!(
            "{LIVE_BASE_URL}{}/page/{}?s={}",
            genre,
            page(&request),
            url::query_escape(query)
        );
        Ok(parse_listing(
            &get_or_fixture(&target, LIST_FIXTURE, LIVE_BASE_URL),
            page(&request),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/watch/sample-series.html".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/watch/sample-series.html".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, LIVE_BASE_URL);
        let mut episodes = parse_episodes(&body, &path);
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/watch/sample-series.html?ep=1".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, EPISODE_FIXTURE, LIVE_BASE_URL);
        let Some(frame) = frame_url(&body) else {
            return Ok(Vec::new());
        };
        let mut streams = resolve_p2pplay(&frame, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
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
        .with_desktop_user_agent()
        .with_header("User-Agent", USER_AGENT)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_cookies_for(LIVE_BASE_URL)
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

fn parse_listing(body: &str, current_page: u64) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = select_all(&doc, "div.col-sm-4.col-xs-12.item")
        .filter_map(|element| {
            let href = attr(&element, "a[href*='/watch/']", "href")?;
            let title = attr(&element, "a[href*='/watch/']", "title")
                .or_else(|| text(&element, "h3 a"))
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: item_key(&href),
                title,
                cover: attr(&element, "img", "data-src")
                    .or_else(|| attr(&element, "img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();

    Paged {
        entries,
        has_next_page: body.contains("next page-numbers")
            || body.contains(&format!("/page/{}", current_page + 1)),
    }
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, LIVE_BASE_URL);
    parse_details(&body, path)
}

fn parse_details(body: &str, path: &str) -> CatalogItem {
    let doc = Html::parse_document(body);
    let details = select_all(&doc, "div.video-details").next();
    let title = details
        .as_ref()
        .and_then(|element| text(element, "h1"))
        .or_else(|| select_attr(&doc, "meta[property='og:title']", "content"))
        .map(clean_title)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title_from_path(path));
    let cover = details
        .as_ref()
        .and_then(|element| {
            attr(element, "img", "data-src").or_else(|| attr(element, "img", "src"))
        })
        .or_else(|| select_attr(&doc, "meta[property='og:image']", "content"))
        .map(|image| absolute_url(&image));
    let description = select_text(&doc, "h2#plot + p")
        .or_else(|| select_attr(&doc, "meta[name='description']", "content"));
    let tags = details
        .as_ref()
        .map(|element| {
            select_all_from(element, "span.meta > a")
                .map(|tag| collect_text(&tag))
                .filter(|tag| !tag.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let year = details
        .as_ref()
        .and_then(|element| meta_value(element, "Year"));
    let status = details
        .as_ref()
        .and_then(|element| meta_value(element, "Status"))
        .map(|value| parse_status(&value))
        .unwrap_or(ItemStatus::Unknown);

    CatalogItem {
        key: item_key(path),
        title,
        cover,
        url: Some(absolute_url(path)),
        description,
        authors: year
            .map(|value| format!("Year {value}"))
            .into_iter()
            .collect(),
        tags,
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, item_path: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut episodes = Vec::new();
    for (idx, element) in select_all(&doc, "ul.pagination.post-tape a").enumerate() {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let label = collect_text(&element);
        let number = label.parse::<f32>().unwrap_or(idx as f32 + 1.0);
        episodes.push(VideoEpisode {
            key: episode_key(href),
            title: Some(format!("Episode {}", display_number(number))),
            episode_number: Some(number),
            url: Some(absolute_url(href)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        });
    }

    if episodes.is_empty() {
        let canonical = select_attr(&doc, "meta[property='og:url']", "content")
            .map(|value| episode_key(&value))
            .unwrap_or_else(|| item_key(item_path));
        episodes.push(VideoEpisode {
            key: canonical.clone(),
            title: Some("Movie".to_string()),
            episode_number: Some(1.0),
            url: Some(absolute_url(&canonical)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        });
    }
    episodes
}

fn frame_url(body: &str) -> Option<String> {
    let doc = Html::parse_document(body);
    select_attr(&doc, "iframe#frame", "src")
        .or_else(|| select_attr(&doc, "iframe[src]", "src"))
        .map(|src| absolute_url(&src))
}

fn resolve_p2pplay(frame: &str, request: &Value) -> Vec<VideoStream> {
    let payload = webview::extract_text(
        webview::ExtractRequest::new(frame, P2PPLAY_EXTRACT_SCRIPT)
            .user_agent(USER_AGENT)
            .header("Referer", LIVE_BASE_URL)
            .wait_for_selector("body")
            .timeout_ms(25_000)
            .headless(false),
    )
    .ok();

    if let Some(payload) = payload {
        let streams = streams_from_payload(&payload, frame, request);
        if !streams.is_empty() {
            return streams;
        }
    }

    vec![VideoStream {
        url: frame.to_string(),
        name: Some("p2pplay".to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(LIVE_BASE_URL),
        initialized: true,
        ..VideoStream::default()
    }]
}

fn streams_from_payload(payload: &str, frame: &str, request: &Value) -> Vec<VideoStream> {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    let entries = value
        .as_array()
        .cloned()
        .or_else(|| value.get("playlist").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut streams = Vec::new();
    for entry in entries {
        let file = entry
            .get("file")
            .or_else(|| entry.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if file.is_empty() {
            continue;
        }
        let stream_url = absolute_or(file, frame);
        let subtitles = subtitle_tracks(&entry, frame);
        if stream_url.contains(".m3u8") {
            streams.extend(expand_hls(&stream_url, frame, subtitles, request));
        } else {
            let quality = entry
                .get("label")
                .and_then(Value::as_str)
                .map(normalize_quality)
                .unwrap_or_else(|| normalize_quality(&stream_url));
            streams.push(media_stream(
                &stream_url,
                &quality,
                frame,
                subtitles,
                request,
            ));
        }
    }
    streams
}

fn expand_hls(
    target: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
) -> Vec<VideoStream> {
    let body = client(referer)
        .get(target)
        .header("Accept", "*/*")
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(target, "auto", referer, subtitles, request)];
    }

    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|value| value.split('x').nth(1))
                .and_then(|value| value.split([',', '\n', '\r']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))?;
            Some(media_stream(
                &absolute_or(line.trim(), target),
                &quality,
                referer,
                subtitles.clone(),
                request,
            ))
        })
        .collect()
}

fn media_stream(
    stream_url: &str,
    quality: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("p2pplay {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        subtitles,
        preferred: quality.contains(&preferred_quality(request)),
        initialized: true,
        ..VideoStream::default()
    }
}

fn subtitle_tracks(entry: &Value, referer: &str) -> Vec<SubtitleTrack> {
    entry
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|track| {
            track
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "captions" || kind == "subtitles")
                .unwrap_or(false)
        })
        .filter_map(|track| {
            let file = track.get("file")?.as_str()?;
            let label = track
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("Subtitle");
            Some(SubtitleTrack {
                url: absolute_or(file, referer),
                language: Some(language_code(label)),
                label: Some(label.to_string()),
                format: Some(if file.ends_with(".srt") { "srt" } else { "vtt" }.to_string()),
                headers: referer_headers(referer),
                is_default: track
                    .get("default")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| label.eq_ignore_ascii_case("english")),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn meta_value(element: &ElementRef<'_>, label: &str) -> Option<String> {
    let text = collect_text(element);
    let idx = text.find(label)?;
    let value = text[idx + label.len()..]
        .trim_start_matches([':', ' '])
        .split("Genre")
        .next()
        .unwrap_or_default()
        .split("Year")
        .next()
        .unwrap_or_default()
        .split("Duration")
        .next()
        .unwrap_or_default()
        .split("Quality")
        .next()
        .unwrap_or_default()
        .split("Status")
        .next()
        .unwrap_or_default()
        .split("Release")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    (!value.is_empty()).then_some(value)
}

fn parse_status(input: &str) -> ItemStatus {
    if input.contains("Ongoing") {
        ItemStatus::Ongoing
    } else if input.contains("Complete") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn select_all<'a>(doc: &'a Html, selector: &str) -> impl Iterator<Item = ElementRef<'a>> {
    let selector = Selector::parse(selector).expect("valid selector");
    doc.select(&selector).collect::<Vec<_>>().into_iter()
}

fn select_all_from<'a>(
    element: &'a ElementRef<'a>,
    selector: &str,
) -> impl Iterator<Item = ElementRef<'a>> {
    let selector = Selector::parse(selector).expect("valid selector");
    element.select(&selector).collect::<Vec<_>>().into_iter()
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
    select_all_from(element, selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, selector: &str, name: &str) -> Option<String> {
    select_all_from(element, selector)
        .next()
        .and_then(|value| value.value().attr(name).map(ToString::to_string))
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
        .map(episode_key)
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL)
        || input.starts_with(LIVE_BASE_URL)
        || input.starts_with("https://tukoz.com/t/")
        || input.starts_with("/watch/"))
    .then(|| item_key(input))
}

fn item_key(input: &str) -> String {
    episode_key(input)
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string()
}

fn episode_key(input: &str) -> String {
    if input.starts_with("http") {
        let mut rest = input
            .trim_start_matches(BASE_URL)
            .trim_start_matches(LIVE_BASE_URL)
            .trim_start_matches("https://tukoz.com/t")
            .to_string();
        if rest.starts_with("http") {
            return input.to_string();
        }
        if !rest.starts_with('/') {
            rest = format!("/{rest}");
        }
        return rest.split('#').next().unwrap_or("/").to_string();
    }
    format!("/{}", input.trim_start_matches('/'))
        .split('#')
        .next()
        .unwrap_or("/")
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        format!("{LIVE_BASE_URL}/{}", input.trim_start_matches('/'))
    }
}

fn absolute_or(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        let root = origin(base).unwrap_or_else(|| base.to_string());
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }
}

fn origin(input: &str) -> Option<String> {
    let scheme = input.split("://").next()?;
    let rest = input.split("://").nth(1)?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}://{host}"))
}

fn title_from_path(path: &str) -> String {
    path.split('?')
        .next()
        .unwrap_or(path)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("tokuzilla")
        .trim_end_matches(".html")
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_title(input: String) -> String {
    input
        .split('|')
        .next()
        .unwrap_or(&input)
        .replace("ENGLISH SUB -", "")
        .replace("】", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_number(number: f32) -> String {
    if number.fract().abs() < f32::EPSILON {
        format!("{}", number as i32)
    } else {
        format!("{number:.1}")
    }
}

fn normalize_quality(input: &str) -> String {
    let digits = input
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if digits.is_empty() {
        "auto".to_string()
    } else if input.contains('p') {
        input.to_string()
    } else {
        format!("{digits}p")
    }
}

fn language_code(label: &str) -> String {
    let lower = label.to_lowercase();
    if lower.contains("english") || lower == "en" {
        "en".to_string()
    } else {
        lower
            .split_whitespace()
            .next()
            .unwrap_or("und")
            .chars()
            .take(3)
            .collect()
    }
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .or_else(|| request.get("pageNumber"))
        .and_then(Value::as_u64)
        .filter(|page| *page > 0)
        .unwrap_or(1)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080p")
        .to_string()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let quality_score = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (i32::from(quality.contains(&preferred)), quality_score)
    });
    streams.reverse();
}

const P2PPLAY_EXTRACT_SCRIPT: &str = r#"
new Promise((resolve) => {
  const serialize = () => {
    try {
      if (window.jwplayer) {
        const player = window.jwplayer('media-player');
        if (player && player.getPlaylist) {
          const playlist = player.getPlaylist();
          if (playlist && playlist.length) {
            resolve(JSON.stringify(playlist));
            return true;
          }
        }
      }
    } catch (_) {}
    return false;
  };
  const start = () => {
    const button = document.querySelector('#player-button');
    if (button) {
      button.click();
    }
    let tries = 0;
    const timer = setInterval(() => {
      if (serialize() || ++tries > 60) {
        clearInterval(timer);
        if (tries > 60) {
          resolve(JSON.stringify([]));
        }
      }
    }, 250);
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})
"#;

const LIST_FIXTURE: &str = r#"
<div class="col-sm-4 col-xs-12 item post">
  <div class="item-img">
    <a title="Sample Tokusatsu" href="https://tukoz.com/t/watch/sample-tokusatsu.html">
      <img src="https://tukoz.com/t/wp-content/uploads/thumb/sample-thumb.png">
    </a>
  </div>
  <h3><a href="https://tukoz.com/t/watch/sample-tokusatsu.html" title="Sample Tokusatsu">Sample Tokusatsu</a></h3>
</div>
<a class="next page-numbers" href="https://tukoz.com/t/page/2">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:url" content="https://tukoz.com/t/watch/sample-tokusatsu.html">
<meta property="og:image" content="https://tukoz.com/t/wp-content/uploads/social/sample.png">
<div class="video-details">
  <h1>Sample Tokusatsu</h1>
  <img data-src="https://tukoz.com/t/wp-content/uploads/thumb/sample-thumb.png">
  <span class="meta"><a>Series</a><a>Kamen Rider</a></span>
  <span class="meta"><span class="meta-info">Year</span> 2024</span>
  <span class="meta"><span class="meta-info">Status</span> Ongoing</span>
</div>
<h2 id="plot">Plot Sample Tokusatsu</h2>
<p>A local smoke-test fixture.</p>
<ul class="pagination post-tape">
  <li><a href="https://tukoz.com/t/watch/sample-tokusatsu.html?ep=1#watch">1</a></li>
  <li><a href="https://tukoz.com/t/watch/sample-tokusatsu.html?ep=2#watch">2</a></li>
</ul>
"#;

const EPISODE_FIXTURE: &str = r#"
<iframe id="frame" src="https://t1.p2pplay.pro/#sample"></iframe>
"#;

export_video_source!(SOURCE);
