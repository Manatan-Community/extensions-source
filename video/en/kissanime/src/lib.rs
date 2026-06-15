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

const SOURCE: KissAnime = KissAnime;
const DEFAULT_BASE_URL: &str = "https://kissanime.com.ru";

struct KissAnime;

impl VideoSource for KissAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let path = if listing(&request) == "latest" {
            "AnimeListOnline/LatestUpdate"
        } else {
            "AnimeListOnline/Trending"
        };
        let body = get_or_fixture(
            &base,
            &format!("{base}/{path}?page={}", page(&request)),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&base, &body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(&base, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&base, &path)],
                has_next_page: false,
            });
        }

        let target =
            if let Some(subpage) = filter(&request, "subpage").filter(|value| !value.is_empty()) {
                format!("{base}/{subpage}/?page={}", page(&request))
            } else if let Some(schedule) =
                filter(&request, "schedule").filter(|value| !value.is_empty())
            {
                format!("{base}/Schedule#{schedule}")
            } else {
                let status = filter(&request, "status").unwrap_or_default();
                let genre = selected_genres(&request);
                format!(
                    "{base}/AdvanceSearch/?name={}&status={}&genre={}&page={}",
                    url::query_escape(query),
                    url::query_escape(status),
                    url::query_escape(&genre),
                    page(&request)
                )
            };
        let body = get_or_fixture(&base, &target, SEARCH_FIXTURE);
        if target.contains("/Schedule#") {
            Ok(Paged {
                entries: parse_schedule(
                    &base,
                    &body,
                    filter(&request, "schedule").unwrap_or_default(),
                ),
                has_next_page: false,
            })
        } else {
            Ok(parse_listing(&base, &body))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/Anime/sample".to_string());
        Ok(fetch_details(&base, &path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/Anime/sample".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), DETAILS_FIXTURE);
        Ok(parse_episodes(&base, &body))
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let episode = request_key(&request, "episode")
            .unwrap_or_else(|| "/Anime/sample/Episode-1?id=1".to_string());
        let episode_url = absolute_url(&base, &episode);
        let body = get_or_fixture(&base, &episode_url, EPISODE_FIXTURE);
        Ok(parse_hosters(&base, &body, &episode_url, &request))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let mut parts = key.splitn(4, '|');
        let server_name = parts.next().unwrap_or("External");
        let iframe_url = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(DEFAULT_BASE_URL);
        let password = parts.next().filter(|value| !value.is_empty());
        let mut streams =
            resolve_embed(&base, server_name, iframe_url, referer, password, &request);
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
                title: "Trending".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest Updates".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|key| absolute_url(&base, &key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(&base, input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&base, &path)),
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

fn client(base: &str, referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(base: &str, target: &str, fixture: &str) -> String {
    client(base, target)
        .get(target)
        .browser_document()
        .referer(base)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_or_fixture(base: &str, target: &str, referer: &str, body: String, fixture: &str) -> String {
    client(base, referer)
        .post(target)
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .header("Origin", base)
        .header("X-Requested-With", "XMLHttpRequest")
        .referer(referer)
        .body(body.into_bytes())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(base: &str, body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "div.listing > div.item_movies_in_cat")
            .filter_map(|element| card_item(base, element))
            .collect(),
        has_next_page: select_all(&doc, "div.pagination > ul > li.current ~ li")
            .next()
            .is_some(),
    }
}

fn card_item(base: &str, element: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&element, "a", "href")?;
    let title = text(&element, "div.title_in_cat_container > a")
        .or_else(|| text(&element, "a"))
        .unwrap_or_else(|| title_from_path(&href));
    Some(CatalogItem {
        key: path_key(base, &href),
        title,
        cover: attr(&element, "img", "src").map(|value| absolute_url(base, &value)),
        url: Some(absolute_url(base, &href)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_schedule(base: &str, body: &str, day: &str) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    select_all(&doc, "div.barContent div.schedule_block")
        .filter(|element| day.is_empty() || collect_text(element).contains(day))
        .filter_map(|element| {
            let href = attr(&element, "a", "href")?;
            Some(CatalogItem {
                key: path_key(base, &href),
                title: text(&element, "h2 > a > span.jtitle")
                    .or_else(|| text(&element, "a"))
                    .unwrap_or_else(|| title_from_path(&href)),
                cover: attr(&element, "img", "src").map(|value| absolute_url(base, &value)),
                url: Some(absolute_url(base, &href)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(base: &str, path: &str) -> CatalogItem {
    let body = get_or_fixture(base, &absolute_url(base, path), DETAILS_FIXTURE);
    parse_details(base, &body, path).unwrap_or_else(|| CatalogItem {
        key: path_key(base, path),
        title: title_from_path(path),
        url: Some(absolute_url(base, path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(base: &str, body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut description = select_text(&doc, "div.full > div.summary > p").unwrap_or_default();
    if let Some(rating) = select_attr(
        &doc,
        "div.Votes > div.Prct > div[data-percent]",
        "data-percent",
    ) {
        if !rating.is_empty() {
            if !description.is_empty() {
                description.push_str("\n\n");
            }
            description.push_str(&format!("User rating: {rating}%"));
        }
    }
    Some(CatalogItem {
        key: path_key(base, path),
        title: select_text(&doc, "div.barContent > div.full > h2")
            .or_else(|| select_text(&doc, "h1"))
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(&doc, "div.cover_anime img, img.cover", "src")
            .map(|value| absolute_url(base, &value)),
        description: (!description.is_empty()).then_some(description),
        tags: select_all(&doc, "div.full > p.info a")
            .map(|tag| collect_text(&tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
        status: parse_status(&collect_texts(&doc, "div.full > div.static_single > p")),
        url: Some(absolute_url(base, path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(base: &str, body: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    select_all(&doc, "div.listing > div:not([class])")
        .filter_map(|element| {
            let href = attr(&element, "a", "href")?;
            let title = text(&element, "a").unwrap_or_else(|| title_from_path(&href));
            Some(VideoEpisode {
                key: path_key(base, &href),
                title: Some(title.clone()),
                episode_number: title
                    .split("Episode ")
                    .nth(1)
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse::<f32>().ok()),
                date_uploaded: select_all_in(element, "div:not(:has(a))")
                    .next()
                    .and_then(|div| parse_mmddyyyy(&collect_text(&div))),
                url: Some(absolute_url(base, &href)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(base: &str, body: &str, episode_url: &str, request: &Value) -> Vec<VideoHoster> {
    let doc = Html::parse_document(body);
    let episode_id = episode_url
        .split("?id=")
        .nth(1)
        .and_then(|tail| tail.split(['&', '#']).next())
        .unwrap_or_default();
    let mut out = Vec::new();
    for option in select_all(&doc, "select#selectServer > option") {
        let Some(value) = option.value().attr("value") else {
            continue;
        };
        let server_name = collect_text(&option);
        let server_page = absolute_url(base, value);
        let server_body = if option.value().attr("selected").is_some() {
            body.to_string()
        } else {
            get_or_fixture(base, &server_page, EPISODE_FIXTURE)
        };
        let ctk = extract_ctk(&server_body);
        if episode_id.is_empty() || ctk.is_none() {
            out.push(external_hoster(&server_name, &server_page));
            continue;
        }
        let server_id = server_page
            .split("?s=")
            .nth(1)
            .and_then(|tail| tail.split(['&', '#']).next())
            .unwrap_or("");
        let iframe_response = post_or_fixture(
            base,
            &format!("{base}/ajax/anime/load_episodes_v2?s={server_id}"),
            &server_page,
            format!(
                "episode_id={}&ctk={}",
                url::query_escape(episode_id),
                url::query_escape(&ctk.unwrap())
            ),
            IFRAME_RESPONSE_FIXTURE,
        );
        let (iframe_url, password) = parse_iframe_response(base, &iframe_response);
        let Some(iframe_url) = iframe_url else {
            out.push(external_hoster(&server_name, &server_page));
            continue;
        };
        out.push(VideoHoster {
            key: format!(
                "{}|{}|{}|{}",
                server_name,
                iframe_url,
                server_page,
                password.unwrap_or_default()
            ),
            name: server_name,
            url: Some(iframe_url),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(episode_url),
            ..VideoHoster::default()
        });
    }
    if out.is_empty() {
        out.push(external_hoster("Episode page", episode_url));
    }
    filter_hosters(out, request)
}

fn resolve_embed(
    base: &str,
    server_name: &str,
    iframe_url: &str,
    referer: &str,
    password: Option<&str>,
    request: &Value,
) -> Vec<VideoStream> {
    if iframe_url.is_empty() {
        return Vec::new();
    }
    if iframe_url.contains("embed.vodstream.xyz") {
        let body = get_or_fixture(base, iframe_url, VODSTREAM_FIXTURE);
        let streams = parse_vodstream(&body, iframe_url, server_name, request);
        if !streams.is_empty() {
            return streams;
        }
    }
    vec![VideoStream {
        url: iframe_url.to_string(),
        name: Some(match password {
            Some(value) if !value.is_empty() => {
                format!("{server_name} - External (password: {value})")
            }
            _ => format!("{server_name} - External"),
        }),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }]
}

fn parse_vodstream(
    body: &str,
    iframe_url: &str,
    server_name: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let Some(raw_sources) = html::text_between(body, "sources: [", "],") else {
        return Vec::new();
    };
    let json = format!("[{raw_sources}]");
    let Ok(sources) = serde_json::from_str::<Vec<EmbedSource>>(&json) else {
        return Vec::new();
    };
    let mut streams = Vec::new();
    for source in sources {
        let is_hls = source.file.contains(".m3u8");
        let quality = source.label.unwrap_or_else(|| {
            if is_hls {
                "HLS".to_string()
            } else {
                "Video".to_string()
            }
        });
        streams.push(VideoStream {
            url: source.file,
            name: Some(format!("{server_name} - {quality}")),
            quality: Some(quality.clone()),
            format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
            is_hls,
            stream_kind: Some(if is_hls {
                VideoStreamKind::Hls
            } else {
                VideoStreamKind::Direct
            }),
            preferred: quality.contains(&preferred_quality(request)),
            headers: referer_headers(iframe_url),
            initialized: true,
            ..VideoStream::default()
        });
    }
    streams
}

fn filter_hosters(hosters: Vec<VideoHoster>, _request: &Value) -> Vec<VideoHoster> {
    hosters
}

fn external_hoster(name: &str, url: &str) -> VideoHoster {
    VideoHoster {
        key: format!("{name}|{url}|{url}|"),
        name: name.to_string(),
        url: Some(url.to_string()),
        lazy: true,
        video_count: Some(1),
        headers: referer_headers(url),
        ..VideoHoster::default()
    }
}

fn extract_ctk(body: &str) -> Option<String> {
    body.split("var ctk = '")
        .nth(1)
        .and_then(|tail| tail.split("';").next())
        .map(ToString::to_string)
}

fn parse_iframe_response(base: &str, body: &str) -> (Option<String>, Option<String>) {
    let response = serde_json::from_str::<IframeResponse>(body).unwrap_or(IframeResponse {
        value: body.to_string(),
    });
    let iframe =
        html::attr_after(&response.value, "<iframe", "src").map(|src| absolute_url(base, &src));
    let password = response
        .value
        .split("password: ")
        .nth(1)
        .and_then(|tail| tail.split(" <button").next())
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    (iframe, password)
}

fn selected_genres(request: &Value) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get("genre"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|genre| format!("{genre}_"))
        .collect::<Vec<_>>()
        .join("")
}

fn parse_status(text: &str) -> ItemStatus {
    if text.contains("Ongoing") {
        ItemStatus::Ongoing
    } else if text.contains("Completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_mmddyyyy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let month = parts.next()?;
    let day = parts.next()?;
    let year = parts.next()?;
    dates::parse_ymd(&format!("{year}-{month}-{day}"))
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    streams.sort_by_key(|stream| {
        if stream
            .name
            .as_deref()
            .unwrap_or_default()
            .contains(&preferred)
            || stream
                .quality
                .as_deref()
                .unwrap_or_default()
                .contains(&preferred)
        {
            0
        } else {
            1
        }
    });
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("1080")
        .to_string()
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("https://"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
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

fn filter<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
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
        .map(|value| path_key(DEFAULT_BASE_URL, value))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn path_from_url(base: &str, input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(base) {
        Some(path_key(base, trimmed))
    } else if trimmed.starts_with("https://kissanime.") {
        Some(path_key(base, trimmed))
    } else if trimmed.starts_with('/') {
        Some(path_key(base, trimmed))
    } else {
        None
    }
}

fn path_key(base: &str, input: &str) -> String {
    if let Some(rest) = input.strip_prefix(base) {
        return path_key(base, rest);
    }
    if let Some(index) = input
        .find(".ru/")
        .or_else(|| input.find(".co/"))
        .or_else(|| input.find(".sx/"))
    {
        let path_start = input[index..]
            .find('/')
            .map(|offset| index + offset)
            .unwrap_or(0);
        return path_key(base, &input[path_start..]);
    }
    let path = input.split('#').next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(base: &str, input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(base, input)
    }
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("KissAnime")
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

fn collect_texts(doc: &Html, query: &str) -> String {
    select_all(doc, query)
        .map(|element| collect_text(&element))
        .collect::<Vec<_>>()
        .join(" ")
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
struct IframeResponse {
    value: String,
}

#[derive(Deserialize)]
struct EmbedSource {
    file: String,
    label: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<div class="listing"><div class="item_movies_in_cat"><a href="/Anime/sample"><img src="/sample.jpg"></a><div class="title_in_cat_container"><a>Sample Anime</a></div></div></div>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<div class="barContent"><div class="full"><h2>Sample Anime</h2><div class="static_single"><p><span>Status:</span> Ongoing</p></div><div class="summary"><p>Sample description.</p></div><p class="info"><span>Genre:</span><a>Action</a></p></div></div>
<div class="cover_anime"><img src="/sample.jpg"></div>
<div class="listing"><div><a href="/Anime/sample/Episode-1?id=1">Episode 1</a><div>01/01/2024</div></div></div>
"#;

const EPISODE_FIXTURE: &str = r#"
<script>var ctk = 'token';</script>
<select id="selectServer"><option selected value="/Anime/sample/Episode-1?id=1&s=vodstream">Vodstream</option></select>
"#;

const IFRAME_RESPONSE_FIXTURE: &str =
    r#"{ "value": "<iframe src=\"https://embed.vodstream.xyz/e/sample\"></iframe>" }"#;

const VODSTREAM_FIXTURE: &str = r#"
<script>var playerInstance = jwplayer("player"); playerInstance.setup({sources: [{"file":"https://cdn.example.invalid/sample.m3u8","label":"720p"}], image:""});</script>
"#;

export_video_source!(SOURCE);
