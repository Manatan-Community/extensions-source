use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    dates, html,
    sdk::{SearchRequest, http::HttpClient},
    url,
    video::referer_headers,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: MyRunningMan = MyRunningMan;
const BASE_URL: &str = "https://www.myrunningman.com";

struct MyRunningMan;

impl VideoSource for MyRunningMan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let path = if listing(&request) == "latest" {
            "episodes/newest"
        } else {
            "episodes/mostwatched"
        };
        let target = format!("{BASE_URL}/{path}/{}", page(&request));
        let body = get_or_fixture(&target, LIST_FIXTURE, BASE_URL);
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
        if let Some(id) = query.strip_prefix("id:").filter(|id| !id.is_empty()) {
            return Ok(Paged {
                entries: vec![fetch_details(&format!("/ep/{id}"))],
                has_next_page: false,
            });
        }
        let response = client(BASE_URL)
            .get(format!(
                "{BASE_URL}/_search.php?q={}",
                url::query_escape(query)
            ))
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(BASE_URL)
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_search(&response),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/ep/1".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/ep/1".to_string());
        let body = get_or_fixture(&absolute_url(&path), DETAILS_FIXTURE, BASE_URL);
        Ok(vec![parse_episode(&body, &path)])
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode")
            .or_else(|| request_key(&request, "item"))
            .unwrap_or_else(|| "/ep/1".to_string());
        let episode_url = absolute_url(&episode);
        let body = get_or_fixture(&episode_url, DETAILS_FIXTURE, BASE_URL);
        Ok(parse_hosters(&body, &episode_url, &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(3, '|');
        let name = parts.next().unwrap_or("External");
        let target = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(BASE_URL);
        if target.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![VideoStream {
            url: target.to_string(),
            name: Some(name.to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            headers: referer_headers(referer),
            initialized: true,
            ..VideoStream::default()
        }])
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
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Most Watched".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Newest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "table > tbody > tr")
            .filter_map(card_item)
            .collect(),
        has_next_page: select_all(&doc, "li > a[aria-label=Next]").next().is_some(),
    }
}

fn card_item(row: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&row, "p > strong > a", "href").or_else(|| attr(&row, "a", "href"))?;
    let title = text(&row, "p > strong > a")
        .or_else(|| text(&row, "a"))
        .unwrap_or_else(|| title_from_path(&href));
    Some(CatalogItem {
        key: path_key(&href),
        title,
        cover: attr(&row, "img", "src").map(|value| absolute_url(&value)),
        url: Some(absolute_url(&href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Vec<SearchResult>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(|result| {
            let id = result.value;
            let numeric_id = id.parse::<u32>().unwrap_or(1);
            let suffix = if numeric_id > 396 { "_temp" } else { "" };
            CatalogItem {
                key: format!("/ep/{id}"),
                title: result.label,
                cover: Some(format!(
                    "{BASE_URL}/assets/epimg/{}{suffix}.jpg",
                    format!("{id:0>3}")
                )),
                url: Some(format!("{BASE_URL}/ep/{id}")),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Completed,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    parse_details(&body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let row = select_all(&doc, "div.row").next();
    Some(CatalogItem {
        key: path_key(path),
        title: select_text(&doc, "div.container h1").unwrap_or_else(|| title_from_path(path)),
        cover: row
            .and_then(|row| attr(&row, "p > img", "src"))
            .map(|value| absolute_url(&value)),
        authors: row
            .map(|row| {
                select_all_in(row, "li > a[href*=guest/]")
                    .map(|link| collect_text(&link))
                    .filter(|value| !value.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        tags: row
            .map(|row| {
                select_all_in(row, "li > a[href*=tag/]")
                    .map(|link| collect_text(&link))
                    .filter(|value| !value.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        description: Some(parse_description(&doc)).filter(|value| !value.is_empty()),
        status: if select_text(&doc, "div.alert").is_some_and(|value| value.contains("Coming soon"))
        {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_description(doc: &Html) -> String {
    select_all(doc, "p")
        .map(|paragraph| collect_text(&paragraph))
        .filter(|value| !value.is_empty())
        .filter(|value| {
            value.contains("Broadcast Date")
                || value.contains("Watches")
                || value.contains("Faves")
                || value.contains("Episode")
        })
        .map(|value| {
            if value.starts_with("Watches") || value.starts_with("Faves") {
                value.split(" (").next().unwrap_or(&value).to_string()
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_episode(body: &str, path: &str) -> VideoEpisode {
    let doc = Html::parse_document(body);
    let number = select_attr(&doc, "div#userepoptions", "data-ep")
        .and_then(|value| value.parse::<f32>().ok())
        .or_else(|| {
            path.trim_end_matches('/')
                .split('/')
                .next_back()?
                .parse::<f32>()
                .ok()
        })
        .unwrap_or(1.0);
    VideoEpisode {
        key: path_key(path),
        title: Some(select_text(&doc, "div.container h1").unwrap_or_else(|| title_from_path(path))),
        episode_number: Some(number),
        date_uploaded: select_all(&doc, "p")
            .map(|paragraph| collect_text(&paragraph))
            .find(|value| value.contains("Broadcast Date"))
            .and_then(|value| value.split(": ").nth(1).map(ToString::to_string))
            .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
            .and_then(|value| dates::parse_ymd(&value)),
        url: Some(absolute_url(path)),
        language: Some("en".to_string()),
        ..VideoEpisode::default()
    }
}

fn parse_hosters(body: &str, episode_url: &str, request: &Value) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    let preferred = request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_hoster"))
        .and_then(Value::as_str)
        .unwrap_or("any");
    select_all(&doc, "a.changePlayer")
        .filter_map(|link| {
            let encoded = link.value().attr("data-url")?;
            let url = decode_hoster_url(encoded)?;
            let name = hoster_name(&url);
            if preferred != "any" && preferred != name {
                return None;
            }
            Some(VideoHoster {
                key: format!("{name}|{url}|{episode_url}"),
                name: name.to_string(),
                url: Some(url),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(episode_url),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn decode_hoster_url(input: &str) -> Option<String> {
    let decoded = input.chars().map(rot13).collect::<String>();
    let (kind, video_id) = decoded.split_at(1);
    match kind {
        "d" => Some(format!("https://dooood.com/e/{video_id}")),
        "m" => Some(format!("https://mixdroop.bz/e/{video_id}")),
        "t" => Some(format!("https://streamtape.com/e/{video_id}")),
        _ => None,
    }
}

fn rot13(ch: char) -> char {
    match ch {
        'a'..='m' | 'A'..='M' => char::from_u32(ch as u32 + 13).unwrap_or(ch),
        'n'..='z' | 'N'..='Z' => char::from_u32(ch as u32 - 13).unwrap_or(ch),
        _ => ch,
    }
}

fn hoster_name(url: &str) -> &'static str {
    if url.contains("doo") {
        "Dood"
    } else if url.contains("mixdro") {
        "MixDrop"
    } else if url.contains("streamtape") {
        "StreamTape"
    } else {
        "External"
    }
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
    let path = input.split('#').next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
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
        .unwrap_or("My Running Man")
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

#[derive(Deserialize)]
struct SearchResult {
    value: String,
    label: String,
}

const LIST_FIXTURE: &str = r#"
<table><tbody><tr><td><p><strong><a href="/ep/1">Episode 1</a></strong></p><img src="/assets/epimg/001.jpg"></td></tr></tbody></table>
"#;

const SEARCH_FIXTURE: &str = r#"[{"value":"1","label":"Episode 1"}]"#;

const DETAILS_FIXTURE: &str = r#"
<div class="container"><h1>Episode 1</h1></div>
<div class="row">
  <p><img src="/assets/epimg/001.jpg"></p>
  <li><a href="/guest/sample">Guest</a></li>
  <li><a href="/tag/funny">Funny</a></li>
  <p><i class="fa"></i>Broadcast Date: 2010-07-11 </p>
  <p><i class="fa"></i>Watches 123 (sample)</p>
</div>
<div id="userepoptions" data-ep="1"></div>
<a class="changePlayer" data-url="qfnzcyr"></a>
"#;

export_video_source!(SOURCE);
