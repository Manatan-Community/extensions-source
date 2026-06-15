use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: AniWave = AniWave;
const DEFAULT_BASE_URL: &str = "https://animewave.to";
const HOSTERS: [&str; 4] = ["megaplay", "vidstream", "vidcloud", "kiwi-stream"];
const TYPES: [&str; 4] = ["Sub", "H-Sub", "Dub", "A-Dub"];

struct AniWave;

impl VideoSource for AniWave {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = page(&request);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "latest-updated"
        } else {
            "most-viewed"
        };
        let body = fetch_or_fixture(
            &format!("{base}/{path}/?page={page}"),
            LIST_FIXTURE,
            &format!("{base}/"),
            &base,
        );
        Ok(parse_listing(&body, &request, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged {
                entries: vec![fetch_details(&path, &request, &base)],
                has_next_page: false,
            });
        }

        let mut target = format!("{base}/filter?keyword={}", url::query_escape(query));
        for (filter_key, query_key) in [
            ("genre", "genre"),
            ("season", "season"),
            ("year", "year"),
            ("term_type", "term_type"),
            ("status", "status"),
            ("language", "language"),
            ("rating", "rating"),
        ] {
            for value in filter_values(&request, filter_key) {
                target.push_str(&format!("&{query_key}[]={}", url::query_escape(&value)));
            }
        }
        if let Some(sort) = filter(&request, "sort").filter(|value| !value.trim().is_empty()) {
            target.push_str(&format!("&sort={}", url::query_escape(&sort)));
        }
        target.push_str(&format!("&page={}", page(&request)));
        if !query.is_empty() {
            target.push_str(&format!("&vrf={}", vrf_encrypt(query)));
        }
        let body = fetch_or_fixture(&target, LIST_FIXTURE, &format!("{base}/"), &base);
        Ok(parse_listing(&body, &request, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = request_raw_key(&request, "item")
            .map(|value| normalize_item_key(&value, &base))
            .unwrap_or_else(|| "/watch/sample-anime".to_string());
        Ok(fetch_details(&key, &request, &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let key = request_raw_key(&request, "item")
            .map(|value| normalize_item_key(&value, &base))
            .unwrap_or_else(|| "/watch/sample-anime#1".to_string());
        let anime_path = key.split('#').next().unwrap_or(&key);
        let id = key
            .split('#')
            .nth(1)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                let body = fetch_or_fixture(
                    &absolute_url(anime_path, &base),
                    DETAILS_FIXTURE,
                    &base,
                    &base,
                );
                anime_id(&body).unwrap_or_else(|| "1".to_string())
            });
        let target = format!("{base}/ajax/episode/list/{id}?vrf={}", vrf_encrypt(&id));
        let body = fetch_ajax_fragment(
            &target,
            EPISODES_FIXTURE,
            &absolute_url(anime_path, &base),
            &base,
        );
        let mut episodes = parse_episodes(&body, anime_path, &base);
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let base = base_url(&request);
        let episode = request_raw_key(&request, "episode")
            .unwrap_or_else(|| "1&epurl=/watch/sample-anime/ep-1".to_string());
        let ids = episode.split('&').next().unwrap_or("1");
        let epurl =
            episode_epurl(&episode).unwrap_or_else(|| "/watch/sample-anime/ep-1".to_string());
        let target = format!("{base}/ajax/server/list?servers={}", url::query_escape(ids));
        let body = fetch_ajax_fragment(
            &target,
            HOSTERS_FIXTURE,
            &absolute_url(&epurl, &base),
            &base,
        );
        Ok(parse_hosters(&body, &episode, &request, &base))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let Some(key) = request_raw_key(&request, "hoster") else {
            return Ok(Vec::new());
        };
        let parts = key.split('|').collect::<Vec<_>>();
        if parts.len() < 4 {
            return Ok(Vec::new());
        }
        let server_id = parts[0];
        let media_type = parts[1];
        let name = parts[2];
        let referer = parts[3];
        let target = format!("{base}/ajax/server?get={server_id}");
        let body = fetch_xhr_or_fixture(&target, SOURCE_FIXTURE, referer, &base);
        let link = serde_json::from_str::<ServerResponse>(&body)
            .ok()
            .map(|response| response.result.url)
            .unwrap_or_default();
        if link.is_empty() {
            return Ok(Vec::new());
        }
        let mut streams =
            if name.contains("kiwi") || link.contains("kwik.") || link.contains("kwik.cx") {
                resolve_kwik(&link, media_type, name, referer, &request)
            } else {
                resolve_player(
                    &resolve_embed_chain(&link, &base),
                    &link,
                    media_type,
                    name,
                    &request,
                    &base,
                )
            };
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
                title: "Most Viewed".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest Updated".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_raw_key(&request, "item")
            .map(|key| normalize_item_key(&key, &base))
            .map(|key| absolute_url(key.split('#').next().unwrap_or(&key), &base)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_raw_key(&request, "episode")
            .and_then(|key| episode_epurl(&key))
            .map(|path| absolute_url(&path, &base)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &request, &base)),
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

fn client(referer: &str, base: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(referer)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, _fixture: &str, referer: &str, base: &str) -> String {
    client(referer, base)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_default()
}

fn fetch_ajax_fragment(target: &str, _fixture: &str, referer: &str, base: &str) -> String {
    let body = client(referer, base)
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    serde_json::from_str::<ResultResponse>(&body)
        .map(|response| response.result)
        .or_else(|_| serde_json::from_str::<HtmlResponse>(&body).map(|response| response.html))
        .unwrap_or(body)
}

fn fetch_xhr_or_fixture(target: &str, _fixture: &str, referer: &str, base: &str) -> String {
    client(referer, base)
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_default()
}

fn fetch_details(key: &str, request: &Value, base: &str) -> CatalogItem {
    let body = fetch_or_fixture(
        &absolute_url(key.split('#').next().unwrap_or(key), base),
        DETAILS_FIXTURE,
        &format!("{base}/"),
        base,
    );
    parse_details(&body, key, request, base).unwrap_or_else(|| fallback_item(key, base))
}

fn parse_listing(body: &str, request: &Value, base: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("class=\"item")
            .skip(1)
            .filter_map(|chunk| parse_card(chunk, request, base))
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let Some(start) = lower.find("pagination") else {
        return false;
    };
    let pagination = &lower[start..];
    let pagination = pagination.split("</ul>").next().unwrap_or(pagination);
    pagination.contains("rel=\"next\"") || pagination.contains("rel='next'")
}

fn parse_card(chunk: &str, request: &Value, base: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "class=\"name", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let path = clean_anime_path(&href, base)?;
    let title = title_from_element(chunk, request).unwrap_or_else(|| title_from_path(&path));
    Some(CatalogItem {
        key: path.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| absolute_url(&image, base)),
        url: Some(absolute_url(&path, base)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: &str, request: &Value, base: &str) -> Option<CatalogItem> {
    let path = clean_anime_path(key.split('#').next().unwrap_or(key), base)?;
    let id = key
        .split('#')
        .nth(1)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| anime_id(body));
    let title_block = html::text_between(body, "class=\"title", "</h1>")
        .or_else(|| html::text_between(body, "class=\"title", "</h2>"))
        .unwrap_or_default();
    let title = title_from_element(
        &format!(
            "class=\"name\" data-jp=\"{}\">{title_block}</a>",
            html::attr(body, "data-jp").unwrap_or_default()
        ),
        request,
    )
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| {
        let stripped = html::strip_tags(&title_block);
        if stripped.is_empty() {
            title_from_path(&path)
        } else {
            stripped
        }
    });
    let description = build_description(body, request);
    Some(CatalogItem {
        key: id
            .filter(|value| !value.is_empty())
            .map(|id| format!("{path}#{id}"))
            .unwrap_or_else(|| path.clone()),
        title,
        cover: poster_image(body).map(|image| absolute_url(&image, base)),
        url: Some(absolute_url(&path, base)),
        description: (!description.is_empty()).then_some(description),
        tags: links_after_label(body, "Genres"),
        authors: links_after_label(body, "Studios"),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&text_after_label(body, "Status").unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn build_description(body: &str, request: &Value) -> String {
    let mut lines = Vec::new();
    let score_position = preference(request, "score_position").unwrap_or_else(|| "top".to_string());
    let score = text_after_label(body, "MAL").and_then(|value| fancy_score(&value));
    if score_position == "top" {
        if let Some(score) = &score {
            lines.push(score.clone());
        }
    }
    if let Some(synopsis) = html::text_between(body, "class=\"content", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        lines.push(synopsis);
    }
    let mut meta = Vec::new();
    for label in ["Type", "Released", "Duration", "Episodes"] {
        if let Some(value) = text_after_label(body, label).filter(|value| !value.is_empty()) {
            meta.push(format!("{label}: {value}"));
        }
    }
    if !meta.is_empty() {
        lines.push(meta.join(" | "));
    }
    let studios = links_after_label(body, "Studios").join(", ");
    let producers = links_after_label(body, "Producers").join(", ");
    if !studios.is_empty() && !producers.is_empty() {
        lines.push(format!("Studio: {studios} (Producers: {producers})"));
    } else if !studios.is_empty() {
        lines.push(format!("Studio: {studios}"));
    } else if !producers.is_empty() {
        lines.push(format!("Producers: {producers}"));
    }
    if let Some(names) = html::text_between(body, "class=\"names", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Other name(s): {names}"));
    }
    if score_position == "bottom" {
        if let Some(score) = score {
            lines.push(score);
        }
    }
    lines.join("\n\n")
}

fn parse_episodes(body: &str, anime_path: &str, base: &str) -> Vec<VideoEpisode> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let link = chunk.split("</a>").next().unwrap_or(chunk);
            let ep_num = html::attr(link, "data-num").unwrap_or_else(|| "0".to_string());
            let ids = html::attr(link, "data-ids")?;
            let title = html::attr(chunk, "title").unwrap_or_default();
            let name = html::text_between(chunk, "class=\"d-title", "</span>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let sub = html::attr(link, "data-sub").as_deref() == Some("1");
            let dub = html::attr(link, "data-dub").as_deref() == Some("1");
            let mut labels = Vec::new();
            if sub {
                labels.push("Sub".to_string());
            }
            if title.to_lowercase().contains("softsub") {
                labels.push("SoftSub".to_string());
            }
            if dub {
                labels.push("Dub".to_string());
            }
            let mut key = format!(
                "{ids}&epurl={}/ep-{}",
                anime_path.trim_end_matches('/'),
                ep_num
            );
            if let Some(mal) = html::attr(link, "data-mal").filter(|value| !value.is_empty()) {
                key.push_str("&mal=");
                key.push_str(&mal);
            }
            if let Some(slug) = html::attr(link, "data-slug").filter(|value| !value.is_empty()) {
                key.push_str("&slug=");
                key.push_str(&slug);
            }
            if let Some(ts) = html::attr(link, "data-timestamp").filter(|value| !value.is_empty()) {
                key.push_str("&ts=");
                key.push_str(&ts);
            }
            let display = if name.is_empty() || name == format!("Episode {ep_num}") {
                format!("Episode {ep_num}")
            } else {
                format!("Episode {ep_num}: {name}")
            };
            let epurl = episode_epurl(&key).unwrap_or_default();
            Some(VideoEpisode {
                key,
                title: Some(display),
                episode_number: ep_num.parse::<f32>().ok(),
                url: Some(absolute_url(&epurl, base)),
                language: Some("en".to_string()),
                labels,
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, episode_key: &str, request: &Value, base: &str) -> Vec<VideoHoster> {
    let enabled_hosters = enabled_values(request, "hoster_selection", &HOSTERS);
    let enabled_types = enabled_values(request, "type_selection", &TYPES);
    let enabled_server_nums = enabled_values(request, "server_number_selection", &["1", "2", "3"]);
    let epurl =
        episode_epurl(episode_key).unwrap_or_else(|| "/watch/sample-anime/ep-1".to_string());
    let episode_url = absolute_url(&epurl, base);
    let mut out = Vec::new();
    for block in body.split("class=\"type").skip(1) {
        let label = html::text_between(block, "<label", "</label>")
            .map(|value| normalize_type_label(&html::strip_tags(&value)))
            .unwrap_or_else(|| {
                normalize_type_label(&html::attr(block, "data-type").unwrap_or_default())
            });
        if !enabled_types
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&label))
        {
            continue;
        }
        let section = block.split("class=\"type").next().unwrap_or(block);
        for li in section.split("<li").skip(1) {
            let Some(server_id) = html::attr(li, "data-link-id") else {
                continue;
            };
            let server_name = li_text(li).to_lowercase();
            if server_name.is_empty()
                || !enabled_hosters
                    .iter()
                    .any(|hoster| server_name.contains(&hoster.to_lowercase()))
                || !enabled_server_nums
                    .iter()
                    .any(|num| get_server_number(&server_name).to_string() == *num)
            {
                continue;
            }
            out.push(VideoHoster {
                key: format!("{server_id}|{label}|{server_name}|{episode_url}"),
                name: format!("{server_name} - {label}"),
                url: Some(episode_url.clone()),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(&episode_url),
                ..VideoHoster::default()
            });
        }
    }
    out
}

fn poster_image(body: &str) -> Option<String> {
    let poster = body
        .find("class=\"poster")
        .or_else(|| body.find("class='poster"))
        .map(|index| &body[index..])
        .and_then(|chunk| chunk.split("</div>").next())
        .unwrap_or(body);
    image_from_html(poster).or_else(|| image_from_html(body))
}

fn image_from_html(body: &str) -> Option<String> {
    for marker in ["<img", "<source"] {
        for attr in ["data-src", "data-original", "src", "poster"] {
            if let Some(value) = html::attr_after(body, marker, attr).filter(is_image_like_url) {
                return Some(value);
            }
        }
    }
    None
}

fn is_image_like_url(value: &String) -> bool {
    let lower = value.to_ascii_lowercase();
    !lower.ends_with(".js")
        && !lower.contains("disqus.com")
        && (lower.contains(".jpg")
            || lower.contains(".jpeg")
            || lower.contains(".png")
            || lower.contains(".webp")
            || lower.contains(".avif")
            || lower.starts_with('/')
            || lower.starts_with("//"))
}

fn li_text(li: &str) -> String {
    let element = li.split("</li>").next().unwrap_or(li);
    let content = element
        .split_once('>')
        .map(|(_, content)| content)
        .unwrap_or(element);
    html::strip_tags(content)
}

fn resolve_player(
    embed_url: &str,
    parent_url: &str,
    media_type: &str,
    name: &str,
    request: &Value,
    base: &str,
) -> Vec<VideoStream> {
    if embed_url.contains(".m3u8") {
        return parse_hls(
            embed_url,
            media_type,
            name,
            parent_url,
            Vec::new(),
            request,
            base,
        );
    }
    let body = fetch_or_fixture(embed_url, "", parent_url, base);
    let host_root = origin(embed_url).unwrap_or_else(|| parent_url.to_string());
    if let Some(data_id) = html::attr(&body, "data-id") {
        let sources_url = format!("{host_root}/stream/getSources?id={data_id}");
        let source_body = fetch_xhr_or_fixture(&sources_url, "", embed_url, base);
        if let Ok(response) = serde_json::from_str::<Value>(&source_body) {
            let subtitles = parse_tracks(&response, &host_root);
            if let Some(source) = extract_source_url(&response) {
                if source.contains(".m3u8") {
                    return parse_hls(
                        &source, media_type, name, &host_root, subtitles, request, base,
                    );
                }
                return vec![media_stream(
                    &source, media_type, name, "direct", &host_root, subtitles, request,
                )];
            }
        }
    }
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
    {
        if src.contains(".m3u8") {
            return parse_hls(&src, media_type, name, embed_url, Vec::new(), request, base);
        }
        return vec![media_stream(
            &src,
            media_type,
            name,
            "direct",
            embed_url,
            Vec::new(),
            request,
        )];
    }
    vec![external_stream(embed_url, media_type, name, request)]
}

fn resolve_kwik(
    embed_url: &str,
    media_type: &str,
    name: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let body = client(referer, DEFAULT_BASE_URL)
        .get(embed_url)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if let Some(source) = unpacked_m3u8(&body) {
        return vec![media_stream(
            &source,
            media_type,
            name,
            "HLS",
            "https://kwik.cx/",
            Vec::new(),
            request,
        )];
    }
    vec![external_stream(embed_url, media_type, name, request)]
}

fn resolve_embed_chain(url: &str, base: &str) -> String {
    let mut current = url.to_string();
    for _ in 0..3 {
        let body = fetch_or_fixture(&current, "", base, base);
        let Some(next) = html::attr_after(&body, "<iframe", "src") else {
            break;
        };
        current = absolute_or(&next, &current);
    }
    current
}

fn parse_hls(
    target: &str,
    media_type: &str,
    name: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
    base: &str,
) -> Vec<VideoStream> {
    let body = client(referer, base)
        .get(target)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(
            target, media_type, name, "auto", referer, subtitles, request,
        )];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split('x').nth(1))
                .and_then(|part| part.split([',', '\n']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "auto".to_string());
            let line = block
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            let stream_url = absolute_or(line.trim(), target);
            Some(media_stream(
                &stream_url,
                media_type,
                name,
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
    media_type: &str,
    name: &str,
    quality: &str,
    referer: &str,
    subtitles: Vec<SubtitleTrack>,
    request: &Value,
) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality} - {media_type}")),
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
        preferred: is_preferred(quality, name, media_type, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, media_type: &str, name: &str, request: &Value) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} - {media_type}")),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        preferred: is_preferred("", name, media_type, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn parse_tracks(response: &Value, referer: &str) -> Vec<SubtitleTrack> {
    response
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|track| {
            track
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| {
                    kind.eq_ignore_ascii_case("captions") || kind.eq_ignore_ascii_case("subtitles")
                })
                .unwrap_or(false)
        })
        .filter_map(|track| {
            let file = track.get("file").and_then(Value::as_str)?;
            let label = track
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("Subtitle");
            Some(SubtitleTrack {
                url: absolute_or(file, referer),
                language: if label.to_lowercase().contains("english") {
                    Some("en".to_string())
                } else {
                    None
                },
                label: Some(label.to_string()),
                format: Some(if file.ends_with(".srt") { "srt" } else { "vtt" }.to_string()),
                headers: referer_headers(referer),
                is_default: label.eq_ignore_ascii_case("english"),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn extract_source_url(response: &Value) -> Option<String> {
    let sources = response.get("sources")?;
    if let Some(file) = sources.get("file").and_then(Value::as_str) {
        return Some(file.to_string());
    }
    if let Some(items) = sources.as_array() {
        for item in items {
            if let Some(file) = item
                .get("file")
                .and_then(Value::as_str)
                .or_else(|| item.as_str())
            {
                return Some(file.to_string());
            }
        }
    }
    sources.as_str().map(ToString::to_string)
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let quality_score = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        let preferred = i32::from(is_preferred(
            quality,
            stream.name.as_deref().unwrap_or_default(),
            stream.name.as_deref().unwrap_or_default(),
            request,
        ));
        (preferred, quality_score)
    });
    streams.reverse();
}

fn is_preferred(quality: &str, server: &str, media_type: &str, request: &Value) -> bool {
    let pref_quality =
        preference(request, "preferred_quality").unwrap_or_else(|| "1080".to_string());
    let pref_server =
        preference(request, "preferred_server").unwrap_or_else(|| "vidstream".to_string());
    let pref_type = preference(request, "preferred_type").unwrap_or_else(|| "Sub".to_string());
    quality.contains(&pref_quality)
        || (server.to_lowercase().contains(&pref_server.to_lowercase())
            && media_type
                .to_lowercase()
                .contains(&pref_type.to_lowercase()))
}

fn enabled_values<const N: usize>(request: &Value, key: &str, defaults: &[&str; N]) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| defaults.iter().map(ToString::to_string).collect())
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "listing": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

fn title_from_element(chunk: &str, request: &Value) -> Option<String> {
    let use_english = preference(request, "preferred_title_lang")
        .map(|value| value == "English")
        .unwrap_or(true);
    let en_title = html::text_between(chunk, "class=\"name", "</a>")
        .or_else(|| html::text_between(chunk, "class=\"title", "</h1>"))
        .or_else(|| html::text_between(chunk, "class=\"title", "</h2>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty());
    let jp_title = html::attr(chunk, "data-jp").filter(|text| !text.trim().is_empty());
    if use_english {
        en_title.or(jp_title)
    } else {
        jp_title.or(en_title)
    }
}

fn links_after_label(body: &str, label: &str) -> Vec<String> {
    let Some(block) = label_block(body, label) else {
        return Vec::new();
    };
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .collect()
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let block = label_block(body, label)?;
    let text = html::strip_tags(block);
    let value = text
        .trim_start_matches(label)
        .trim_start_matches(':')
        .trim()
        .to_string();
    (!value.is_empty()).then_some(value)
}

fn label_block<'a>(body: &'a str, label: &str) -> Option<&'a str> {
    let start = body
        .find(&format!("{label}:"))
        .or_else(|| body.find(label))?;
    Some(body[start..].split("</div>").next().unwrap_or_default())
}

fn anime_id(body: &str) -> Option<String> {
    html::attr(body, "data-id").or_else(|| html::attr(body, "data-tip"))
}

fn parse_status(input: &str) -> ItemStatus {
    match input.trim() {
        "Ongoing Anime" | "Currently Airing" => ItemStatus::Ongoing,
        "Finished Airing" | "Completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn fancy_score(input: &str) -> Option<String> {
    let score = input.trim().parse::<f32>().ok()?;
    if score <= 0.0 {
        return None;
    }
    let stars = ((score / 2.0).round() as usize).min(5);
    Some(format!(
        "{}{} {}",
        "*".repeat(stars),
        "-".repeat(5 - stars),
        score
    ))
}

fn normalize_type_label(input: &str) -> String {
    match input.trim().to_uppercase().as_str() {
        "SUB" => "Sub".to_string(),
        "H-SUB" => "H-Sub".to_string(),
        "DUB" => "Dub".to_string(),
        "A-DUB" => "A-Dub".to_string(),
        other => {
            let lower = other.to_lowercase();
            let mut chars = lower.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_else(|| "Sub".to_string())
        }
    }
}

fn get_server_number(server_name: &str) -> i32 {
    server_name
        .split('-')
        .next_back()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(1)
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
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
        .map(ToString::to_string)
}

fn path_from_url(input: &str, base: &str) -> Option<String> {
    if input.starts_with(base)
        || input.starts_with(DEFAULT_BASE_URL)
        || input.starts_with("https://aniwave.")
        || input.starts_with("https://animewave.")
        || input.starts_with("/watch/")
    {
        clean_anime_path(input, base)
    } else {
        None
    }
}

fn normalize_item_key(input: &str, base: &str) -> String {
    let id = input.split('#').nth(1).filter(|value| !value.is_empty());
    let path = clean_anime_path(input.split('#').next().unwrap_or(input), base)
        .unwrap_or_else(|| "/watch/sample-anime".to_string());
    id.map(|id| format!("{path}#{id}")).unwrap_or(path)
}

fn clean_anime_path(input: &str, base: &str) -> Option<String> {
    let without_base = input
        .strip_prefix(base)
        .or_else(|| input.strip_prefix(DEFAULT_BASE_URL))
        .or_else(|| {
            input
                .strip_prefix("https://aniwave.id")
                .or_else(|| input.strip_prefix("https://aniwave.best"))
                .or_else(|| input.strip_prefix("https://aniwave.ro"))
        })
        .unwrap_or(input);
    let mut path = without_base
        .split(['?', '#'])
        .next()
        .unwrap_or(without_base);
    if let Some((prefix, suffix)) = path.rsplit_once("/ep-") {
        if suffix.chars().all(|ch| ch.is_ascii_digit()) {
            path = prefix;
        }
    }
    let path = format!("/{}", path.trim_matches('/'));
    path.starts_with("/watch/").then_some(path)
}

fn episode_epurl(input: &str) -> Option<String> {
    input
        .split("epurl=")
        .nth(1)
        .map(|value| value.split('&').next().unwrap_or(value).to_string())
}

fn absolute_url(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(base, input)
    }
}

fn absolute_or(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        return input.to_string();
    }
    if input.starts_with("//") {
        return format!("https:{input}");
    }
    let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        input.trim_start_matches('/')
    )
}

fn origin(input: &str) -> Option<String> {
    let scheme_end = input.find("://")? + 3;
    let host_end = input[scheme_end..]
        .find('/')
        .map(|offset| scheme_end + offset)
        .unwrap_or(input.len());
    Some(format!("{}/", input[..host_end].trim_end_matches('/')))
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("AniWave")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
    else {
        return Vec::new();
    };
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn base_url(request: &Value) -> String {
    preference(request, "custom_domain")
        .filter(|value| !value.trim().is_empty())
        .or_else(|| preference(request, "preferred_domain"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn fallback_item(key: &str, base: &str) -> CatalogItem {
    let path = clean_anime_path(key, base).unwrap_or_else(|| "/watch/sample-anime".to_string());
    CatalogItem {
        key: key.to_string(),
        title: title_from_path(&path),
        url: Some(absolute_url(&path, base)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn vrf_encrypt(input: &str) -> String {
    let mut vrf = exchange(input, "AP6GeR8H0lwUz1", "UAz8Gwl10P6ReH");
    vrf = rc4_encrypt("ItFKjuWokn4ZpB", &vrf);
    vrf = rc4_encrypt("fOyt97QWFB3", &vrf);
    vrf = exchange(&vrf, "1majSlPQd2M5", "da1l2jSmP5QM");
    vrf = exchange(&vrf, "CPYvHj09Au3", "0jHA9CPYu3v");
    vrf = vrf.chars().rev().collect::<String>();
    vrf = rc4_encrypt("736y1uTJpBLUX", &vrf);
    url::query_escape(&URL_SAFE.encode(vrf.as_bytes()))
}

fn exchange(input: &str, from: &str, to: &str) -> String {
    input
        .chars()
        .map(|ch| {
            from.find(ch)
                .and_then(|index| to.chars().nth(index))
                .unwrap_or(ch)
        })
        .collect()
}

fn rc4_encrypt(key: &str, input: &str) -> String {
    let mut s = [0u8; 256];
    for (i, value) in s.iter_mut().enumerate() {
        *value = i as u8;
    }
    let key = key.as_bytes();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) & 0xff;
        s.swap(i, j);
    }
    let mut i = 0usize;
    j = 0;
    let mut out = Vec::with_capacity(input.len());
    for byte in input.as_bytes() {
        i = (i + 1) & 0xff;
        j = (j + s[i] as usize) & 0xff;
        s.swap(i, j);
        let k = s[((s[i] as usize + s[j] as usize) & 0xff) as usize];
        out.push(byte ^ k);
    }
    URL_SAFE.encode(out)
}

fn unpacked_m3u8(body: &str) -> Option<String> {
    body.split(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"' || ch == '\\')
        .find(|part| part.starts_with("http") && part.contains(".m3u8"))
        .map(ToString::to_string)
}

#[derive(Deserialize)]
struct HtmlResponse {
    html: String,
}

#[derive(Deserialize)]
struct ResultResponse {
    result: String,
}

#[derive(Deserialize)]
struct ServerResponse {
    result: ServerResult,
}

#[derive(Deserialize)]
struct ServerResult {
    url: String,
}

const LIST_FIXTURE: &str = r#"
<div class="ani items"><div class="item"><div class="poster"><img data-src="/poster.jpg"></div><a class="name" href="/watch/sample-anime" data-jp="Sample Anime JP">Sample Anime</a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<main data-id="1"><h1 class="title" data-jp="Sample Anime JP">Sample Anime</h1><div class="poster"><img src="/poster.jpg"></div><div class="synopsis"><div class="shorting"><div class="content">Sample overview.</div></div></div><div class="bmeta"><div class="meta"><div>Status: <span>Currently Airing</span></div><div>Genres: <span><a>Action</a></span></div><div>Studios: <span><a>Sample Studio</a></span></div><div>MAL: <span>8.2</span></div></div></div>
"#;

const EPISODES_FIXTURE: &str = r#"
<div class="episodes"><ul><li title="Release: 2024/01/01 00:00"><a data-num="1" data-ids="1" data-sub="1" data-dub="0" data-mal="1" data-slug="sample" data-timestamp="1"><span class="d-title">Beginning</span></a></li></ul></div>
"#;

const HOSTERS_FIXTURE: &str = r#"
<div class="servers"><div class="type" data-type="sub"><label>SUB</label><ul><li data-link-id="1">vidstream-1</li><li data-link-id="2">megaplay-1</li></ul></div></div>
"#;

const SOURCE_FIXTURE: &str = r#"{"result":{"url":"https://example.com/embed/sample"}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poster_image_stays_inside_poster_block() {
        let body = r#"
        <div class="poster"><img data-src="https://cdn.example/poster.webp"></div>
        <script src="https://animewave.to/animixplay-fqsqfvdf4u.disqus.com/embed.js"></script>
        "#;

        assert_eq!(
            poster_image(body).as_deref(),
            Some("https://cdn.example/poster.webp")
        );
    }

    #[test]
    fn poster_image_rejects_script_fallbacks() {
        let body = r#"<main><script src="https://animewave.to/animixplay-fqsqfvdf4u.disqus.com/embed.js"></script></main>"#;

        assert_eq!(poster_image(body), None);
    }

    #[test]
    fn hoster_parser_uses_li_text_not_attributes() {
        let body = r#"
        <div class="servers"><div class="type" data-type="sub"><label>SUB</label><ul>
        <li data-ep-id="114623" data-cmid="animixplay-fqsqfvdf4u" data-sv-id="e54" data-link-id="abc">vidstream-2</li>
        </ul></div></div>
        "#;
        let hosters = parse_hosters(
            body,
            "1&epurl=/watch/sample-anime/ep-1",
            &json!({}),
            DEFAULT_BASE_URL,
        );

        assert_eq!(hosters.len(), 1);
        assert_eq!(hosters[0].name, "vidstream-2 - Sub");
        assert!(hosters[0].key.contains("|vidstream-2|"));
        assert!(!hosters[0].name.contains("data-ep-id"));
    }

    #[test]
    fn listing_detects_current_aniwave_next_link() {
        let body = r#"
        <div class="ani items"><div class="item"><a class="name" href="/watch/sample-anime">Sample Anime</a></div></div>
        <ul class="pagination">
            <li class="page-item active"><a class="page-link">1</a></li>
            <li class="page-item"><a title="Page 2" class="page-link" href="/most-viewed?page=2">2</a></li>
            <li class="page-item"><a class="page-link" rel="next" href="/most-viewed?page=2">›</a></li>
        </ul>
        "#;

        let page = parse_listing(body, &json!({}), DEFAULT_BASE_URL);

        assert!(page.has_next_page);
        assert_eq!(page.entries.len(), 1);
    }

    #[test]
    fn listing_stops_on_current_aniwave_last_page() {
        let body = r#"
        <div class="ani items"><div class="item"><a class="name" href="/watch/sample-anime">Sample Anime</a></div></div>
        <ul class="pagination">
            <li class="page-item"><a class="page-link" rel="prev" href="/most-viewed?page=294">‹</a></li>
            <li class="page-item"><a title="Page 294" class="page-link" href="/most-viewed?page=294">294</a></li>
            <li class="page-item active"><a class="page-link">295</a></li>
        </ul>
        "#;

        let page = parse_listing(body, &json!({}), DEFAULT_BASE_URL);

        assert!(!page.has_next_page);
        assert_eq!(page.entries.len(), 1);
    }

    #[test]
    fn origin_keeps_trailing_slash_for_referer() {
        assert_eq!(
            origin("https://megaplay.buzz/stream/s-2/131394/sub").as_deref(),
            Some("https://megaplay.buzz/")
        );
    }
}

export_video_source!(SOURCE);
