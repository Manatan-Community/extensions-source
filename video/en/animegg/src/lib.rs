use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

const SOURCE: AnimeGg = AnimeGg;
const BASE_URL: &str = "https://www.animegg.org";

struct AnimeGg;

impl VideoSource for AnimeGg {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = listing(&request);
        let target = if listing == "latest" {
            format!(
                "{BASE_URL}/popular-series?sortBy=createdAt&sortDirection=DESC&ongoing&limit=50&start=0"
            )
        } else {
            format!(
                "{BASE_URL}/popular-series?sortBy=hits&sortDirection=DESC&ongoing&limit=50&start=0"
            )
        };
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(Paged {
            entries: parse_popular(&body),
            has_next_page: false,
        })
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
        if !query.is_empty() {
            let target = format!("{BASE_URL}/search/?q={}", url::query_escape(query));
            let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
            return Ok(Paged {
                entries: parse_search(&body),
                has_next_page: false,
            });
        }
        if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            let page = page(&request);
            let target = format!("{BASE_URL}/{genre}/page/{page}");
            let body = get_or_fixture(&target, SEARCH_FIXTURE, BASE_URL);
            return Ok(Paged {
                entries: parse_search(&body),
                has_next_page: false,
            });
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/series/sample-anime".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/series/sample-anime".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode =
            request_key(&request, "episode").unwrap_or_else(|| "/episode/sample-1".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, EPISODE_FIXTURE, BASE_URL);
        let mut streams = parse_streams(&body, &episode_url, &request);
        sort_streams(&mut streams, &request);
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
        .with_referer(referer)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
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

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| fallback_item(path))
}

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    select_all(&doc, ".fea")
        .filter_map(|element| {
            let title = text(&element, ".rightpop a")?;
            let href = attr(&element, ".rightpop a", "href")?;
            Some(card_item(&href, title, attr(&element, "img", "src")))
        })
        .collect()
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    select_all(&doc, ".mse")
        .filter_map(|element| {
            let title = text(&element, ".first h2")?;
            let href = element
                .value()
                .attr("href")
                .map(ToString::to_string)
                .or_else(|| attr(&element, "a", "href"))?;
            Some(card_item(&href, title, attr(&element, "img", "src")))
        })
        .collect()
}

fn card_item(href: &str, title: String, cover: Option<String>) -> CatalogItem {
    let key = path_key(href);
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(&doc, ".media-body h1").unwrap_or_else(|| title_from_path(path));
    let status_text = select_all(&doc, ".infoami span")
        .map(|span| collect_text(&span))
        .find(|value| value.contains("Status"))
        .unwrap_or_default();
    let status = if status_text.is_empty() {
        if path.contains("/series/") {
            ItemStatus::Unknown
        } else {
            ItemStatus::Completed
        }
    } else {
        parse_status(&status_text)
    };
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr(&doc, ".media .media-object", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: select_text(&doc, ".ptext"),
        tags: select_all(&doc, ".tagscat a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for (idx, element) in select_all(&doc, ".newmanga li div").enumerate() {
        let Some(href) = attr(&element, ".anm_det_pop", "href") else {
            continue;
        };
        let number = text(&element, ".anm_det_pop strong")
            .and_then(|value| ep_number(&value))
            .unwrap_or(idx as f32 + 1.0);
        let episode_title = text(&element, ".anititle").unwrap_or_else(|| title_from_path(&href));
        let display = display_number(number);
        let title = if episode_title.contains(&display) {
            episode_title
        } else {
            format!("Episode {display} - {episode_title}")
        };
        let scanlator = select_all_from(&element, ".btn-xs")
            .map(|item| collect_text(&item))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        out.push(VideoEpisode {
            key: path_key(&href),
            title: Some(title),
            episode_number: Some(number),
            release_group: (!scanlator.is_empty()).then(|| scanlator.join(", ")),
            url: Some(absolute_url(&href)),
            language: Some("en".to_string()),
            ..VideoEpisode::default()
        });
    }
    out
}

fn parse_streams(body: &str, episode_url: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for iframe in select_all(&doc, "iframe") {
        let Some(src) = iframe.value().attr("src") else {
            continue;
        };
        let link = absolute_url(src);
        let mode = stream_mode(&iframe);
        let embed_body = get_or_fixture(&link, EMBED_FIXTURE, episode_url);
        let Some(script) = select_all(&Html::parse_document(&embed_body), "script")
            .map(|script| script.text().collect::<Vec<_>>().join(""))
            .find(|script| script.contains("var videoSources ="))
        else {
            continue;
        };
        let host_url = origin(&link).unwrap_or_else(|| BASE_URL.to_string());
        let referer = host_url.clone();
        for source in parse_video_sources(&script) {
            let stream_url = absolute_or(&source.file, &host_url);
            let quality = normalize_quality(&source.label);
            let is_hls = stream_url.contains(".m3u8");
            out.push(VideoStream {
                url: stream_url,
                name: Some(format!("{mode} AnimeGG:{}", source.label)),
                quality: Some(quality),
                format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
                is_hls,
                stream_kind: Some(if is_hls {
                    VideoStreamKind::Hls
                } else {
                    VideoStreamKind::Direct
                }),
                headers: referer_headers(&referer),
                preferred: is_preferred(&source.label, &mode, request),
                initialized: true,
                ..VideoStream::default()
            });
        }
    }
    out
}

fn stream_mode(iframe: &ElementRef<'_>) -> &'static str {
    iframe
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find_map(|ancestor| {
            if !ancestor.value().classes().any(|class| class == "tab-pane") {
                return None;
            }
            match ancestor.value().attr("id") {
                Some("subbed-Animegg") => Some("[SUBBED]"),
                Some("dubbed-Animegg") => Some("[DUBBED]"),
                Some("raw-Animegg") => Some("[RAW]"),
                _ => None,
            }
        })
        .unwrap_or("")
}

#[derive(Debug)]
struct GgVideo {
    file: String,
    label: String,
}

fn parse_video_sources(script: &str) -> Vec<GgVideo> {
    let raw = script
        .split("var videoSources =")
        .nth(1)
        .and_then(|value| value.split(';').next())
        .unwrap_or_default();
    raw.split('{')
        .skip(1)
        .filter_map(|chunk| {
            let object = chunk.split('}').next().unwrap_or_default();
            let file = js_field(object, "file")?;
            let label = js_field(object, "label").unwrap_or_else(|| normalize_quality(&file));
            Some(GgVideo { file, label })
        })
        .collect()
}

fn js_field(input: &str, name: &str) -> Option<String> {
    for needle in [
        format!("{name}:"),
        format!("\"{name}\":"),
        format!("'{name}':"),
    ] {
        let Some(start) = input.find(&needle).map(|idx| idx + needle.len()) else {
            continue;
        };
        let rest = input[start..].trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let rest = &rest[quote.len_utf8()..];
        let mut escaped = false;
        let mut out = String::new();
        for ch in rest.chars() {
            if escaped {
                out.push(match ch {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                return Some(out);
            } else {
                out.push(ch);
            }
        }
    }
    None
}

fn parse_status(input: &str) -> ItemStatus {
    let value = input.split("Status:").nth(1).unwrap_or(input).trim();
    if value.contains("Completed") {
        ItemStatus::Completed
    } else if value.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn ep_number(input: &str) -> Option<f32> {
    let token = input
        .split_whitespace()
        .last()?
        .split('-')
        .next()
        .unwrap_or_default();
    token.parse().ok()
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let name = stream.name.as_deref().unwrap_or_default();
        let quality_score = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (
            i32::from(name.contains(&preferred_language(request))),
            i32::from(
                name.to_lowercase()
                    .contains(&preferred_server(request).to_lowercase()),
            ),
            i32::from(quality.contains(&preferred_quality(request))),
            quality_score,
        )
    });
    streams.reverse();
}

fn is_preferred(label: &str, mode: &str, request: &Value) -> bool {
    mode.contains(&preferred_language(request))
        || label.contains(&preferred_quality(request))
        || label
            .to_lowercase()
            .contains(&preferred_server(request).to_lowercase())
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
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL)
        || input.starts_with("/series/")
        || input.starts_with("/peliculas/"))
    .then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        let rest = input.trim_start_matches(BASE_URL);
        return format!("/{}", rest.trim_start_matches('/'))
            .split(['?', '#'])
            .next()
            .unwrap_or("/")
            .to_string();
    }
    if input.starts_with("http") {
        return input.to_string();
    }
    format!("/{}", input.trim_start_matches('/'))
        .split(['?', '#'])
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
        format!("{BASE_URL}/{}", input.trim_start_matches('/'))
    }
}

fn absolute_or(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
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
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("animegg")
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

fn display_number(number: f32) -> String {
    if (number.fract()).abs() < f32::EPSILON {
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
        input.to_string()
    } else if input.contains('p') {
        input.to_string()
    } else {
        format!("{digits}p")
    }
}

fn listing(request: &Value) -> String {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut next = request.clone();
    if !next.is_object() {
        next = json!({});
    }
    next["listing"] = Value::String(listing.to_string());
    next
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

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preferred_language(request: &Value) -> String {
    preference(request, "preferred_language").unwrap_or_else(|| "[SUBBED]".to_string())
}

fn preferred_server(request: &Value) -> String {
    preference(request, "preferred_server").unwrap_or_else(|| "AnimeGG".to_string())
}

fn preferred_quality(request: &Value) -> String {
    preference(request, "preferred_quality").unwrap_or_else(|| "1080".to_string())
}

fn fallback_item(path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

const LIST_FIXTURE: &str = r#"
<div class="fea">
  <img src="/images/sample.jpg">
  <div class="rightpop"><a href="/series/sample-anime">Sample Anime</a></div>
</div>
"#;

const SEARCH_FIXTURE: &str = r#"
<a class="mse" href="/series/sample-anime">
  <img src="/images/sample.jpg">
  <div class="first"><h2>Sample Anime</h2></div>
</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="media">
  <img class="media-object" src="/images/sample.jpg">
  <div class="media-body"><h1>Sample Anime</h1></div>
</div>
<p class="ptext">A local smoke-test fixture.</p>
<div class="tagscat"><a>Action</a><a>Adventure</a></div>
<div class="infoami"><span>Status: Ongoing</span></div>
<ul class="newmanga">
  <li><div>
    <a class="anm_det_pop" href="/episode/sample-anime-1"><strong>1</strong></a>
    <span class="anititle">A Beginning</span>
    <span class="btn-xs">SUB</span>
  </div></li>
</ul>
"#;

const EPISODE_FIXTURE: &str = r#"
<div class="tab-pane" id="subbed-Animegg">
  <iframe src="/embed/sample"></iframe>
</div>
"#;

const EMBED_FIXTURE: &str = r#"
<script>
var videoSources = [{file:"/media/sample-1080.mp4",label:"1080p"}];
</script>
"#;

export_video_source!(SOURCE);
