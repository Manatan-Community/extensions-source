use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, Paged, SubtitleTrack,
    UrlResolveResult, VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult,
    export_video_source, source::VideoSource, webview,
};
use manatan_shared::{
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SOURCE: OneTwoThreeCine = OneTwoThreeCine;
const BASE_URL: &str = "https://123cine.to";
const ENC_MOVIES_URL: &str = "https://enc-dec.app/api/enc-movies-flix";
const DEC_MOVIES_URL: &str = "https://enc-dec.app/api/dec-movies-flix";
const DEC_RAPID_URL: &str = "https://enc-dec.app/api/dec-rapid";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Mobile Safari/537.36";
const SERVERS: [&str; 2] = ["Server 1", "Server 2"];

struct OneTwoThreeCine;

impl VideoSource for OneTwoThreeCine {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_page(LIST_FIXTURE, BASE_URL));
        }
        let base = base_url(&request);
        let target = if listing(&request) == "latest" {
            format!("{base}/updates?page={}", page(&request))
        } else {
            format!("{base}/browser?sort=trending&page={}", page(&request))
        };
        Ok(parse_page(
            &get_or_fixture(&target, LIST_FIXTURE, &base),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let base = base_url(&request);
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged {
                entries: vec![fetch_details(&path, &request)],
                has_next_page: false,
            });
        }
        if query.is_empty() && request.get("filters").is_none() {
            return self.list(request);
        }

        let target = search_url(&base, query, &request);
        Ok(parse_page(
            &get_or_fixture(&target, LIST_FIXTURE, &base),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-movie".to_string());
        Ok(fetch_details(&path, &request))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/watch/sample-movie".to_string());
        let anime_id =
            fetch_title_id(&path, &request).unwrap_or_else(|| "sample-title".to_string());
        let enc = encrypt(&anime_id).unwrap_or_else(|| "sample".to_string());
        let base = base_url(&request);
        let target = format!("{base}/api/v1/titles/{anime_id}/episodes?_={enc}");
        let body = api_get_or_fixture(&target, EPISODES_FIXTURE, &format!("{base}/watch"), &base);
        Ok(parse_episodes(&body))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode_id =
            request_key(&request, "episode").unwrap_or_else(|| "sample-episode".to_string());
        let enc = encrypt(&episode_id).unwrap_or_else(|| "sample".to_string());
        let base = base_url(&request);
        let target = format!("{base}/api/v1/episodes/{episode_id}?_={enc}");
        let body = api_get_or_fixture(&target, SERVERS_FIXTURE, &format!("{base}/watch"), &base);
        let response = serde_json::from_str::<EpisodeServersResponse>(&body)
            .unwrap_or_else(|_| serde_json::from_str(SERVERS_FIXTURE).unwrap_or_default());
        if response.status != "ok" {
            return Ok(Vec::new());
        }

        let enabled = enabled_servers(&request);
        let mut streams = Vec::new();
        for link in response.result.links {
            if !enabled.iter().any(|server| server == &link.name) {
                continue;
            }
            let link_enc = encrypt(&link.id).unwrap_or_else(|| "sample".to_string());
            let target = format!("{base}/api/v1/links/{}?_={link_enc}", link.id);
            let body = api_get_or_fixture(&target, LINK_FIXTURE, &format!("{base}/watch"), &base);
            let link_response = serde_json::from_str::<LinkResponse>(&body)
                .unwrap_or_else(|_| serde_json::from_str(LINK_FIXTURE).unwrap_or_default());
            if link_response.status != "ok" {
                continue;
            }
            let Some(iframe_url) = decrypt_link(&link_response.result) else {
                continue;
            };
            streams.extend(rapidshare_streams(&iframe_url, &link.name, &request));
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
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|path| absolute_url(&base, &path)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &request)),
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
        .with_header("User-Agent", USER_AGENT)
        .with_header("Accept-Language", "en-US,en;q=0.9")
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
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

fn api_get_or_fixture(target: &str, fixture: &str, referer: &str, base: &str) -> String {
    client(base)
        .get(target)
        .xhr()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(path: &str, request: &Value) -> CatalogItem {
    let base = base_url(request);
    let body = get_or_fixture(&absolute_url(&base, path), DETAILS_FIXTURE, &base);
    parse_details(&body, &path_key(path), request).unwrap_or_else(|| fallback_item(path, &base))
}

fn parse_page(body: &str, base: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut seen = BTreeSet::new();
    let entries = select_all(&doc, "div.item > div.inner")
        .filter_map(|item| {
            let poster = select_all_in(item, "a.poster").next()?;
            let href = attr(&poster, "href")?;
            let key = path_key(&href);
            if !key.starts_with("/watch/") || !seen.insert(key.clone()) {
                return None;
            }
            let title = text_of_first(item, "div.detail > div.title")
                .unwrap_or_else(|| title_from_path(&key));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: attr_of_first(item, "a.poster img", "src")
                    .map(|image| absolute_url(base, &image)),
                url: Some(absolute_url(base, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: select_all(&doc, "ul.pagination a[rel=next]")
            .next()
            .is_some(),
    }
}

fn parse_details(body: &str, path: &str, request: &Value) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let head = select_all(&doc, ".head-movie-wrapper").next()?;
    let title = text_of_first(head, "h1.title").unwrap_or_else(|| title_from_path(path));
    let is_movie = select_all_in(head, ".metadata .dot")
        .any(|node| collect_text(&node).eq_ignore_ascii_case("Movie"));
    let meta_foot = select_all(&doc, ".mini-meta-foot").next();
    let tags = meta_foot
        .map(|meta| collect_texts_in(meta, "a[href^='/genre/'], a[href^=\"/genre/\"]"))
        .unwrap_or_default();
    let country = meta_foot
        .map(|meta| {
            collect_texts_in(meta, "a[href^='/country/'], a[href^=\"/country/\"]").join(", ")
        })
        .unwrap_or_default();
    let released = meta_value(meta_foot, "Released:");
    let quality = meta_value(meta_foot, "Quality:");
    let duration = select_all_in(head, ".metadata .dot")
        .map(|node| collect_text(&node))
        .find(|text| text.to_ascii_lowercase().ends_with("min"))
        .unwrap_or_default();
    let desc = select_text(&doc, ".movie-info .desc");
    let director = side_meta_value(&doc, "Director");
    let casts = side_meta_value(&doc, "Casts");
    let productions = side_meta_value(&doc, "Productions");
    let rating = highlight_values(&doc)
        .into_iter()
        .nth(1)
        .unwrap_or_default();
    let score = highlight_values(&doc)
        .first()
        .map(|value| value.trim_start_matches("IMDb").trim().to_string())
        .and_then(|value| fancy_score(&value));
    let score_pos = pref(request, "score_position", "top");
    let mut description = String::new();
    if score_pos == "top" {
        if let Some(score) = &score {
            push_line(&mut description, score);
            description.push('\n');
        }
    }
    if let Some(desc) = desc {
        push_line(&mut description, &desc);
    }
    push_meta(&mut description, "Quality", &quality);
    push_meta(&mut description, "Country", &country);
    push_meta(&mut description, "Released", &released);
    push_meta(&mut description, "Duration", &duration);
    push_meta(&mut description, "Rating", &rating);
    push_meta(&mut description, "Director", &director);
    push_meta(&mut description, "Casts", &casts);
    if score_pos == "bottom" {
        if let Some(score) = &score {
            if !description.trim().is_empty() {
                description.push('\n');
            }
            push_line(&mut description, score);
        }
    }
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover: select_attr(&doc, ".detail-start .poster img", "src")
            .or_else(|| select_attr(&doc, "meta[property='og:image']", "content"))
            .map(|image| absolute_url(&base_url(request), &image)),
        url: Some(absolute_url(&base_url(request), path)),
        authors: productions
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags,
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: if is_movie {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str) -> Vec<VideoEpisode> {
    let response = serde_json::from_str::<EpisodesResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(EPISODES_FIXTURE).unwrap_or_default());
    if response.status != "ok" {
        return Vec::new();
    }
    let is_movie = response.result.title.media_type == "movie";
    let mut episodes = Vec::new();
    for season in response.result.seasons {
        for episode in season.episodes {
            let title = if is_movie {
                episode.detail_name.unwrap_or_else(|| "Movie".to_string())
            } else {
                let detail = episode
                    .detail_name
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| format!(": {value}"))
                    .unwrap_or_default();
                format!(
                    "S{} E{}{}",
                    season.number,
                    display_number(episode.number),
                    detail
                )
            };
            episodes.push(VideoEpisode {
                key: episode.id,
                title: Some(title),
                episode_number: Some(if is_movie { 1.0 } else { episode.number }),
                season_number: (!is_movie).then_some(season.number as f32),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            });
        }
    }
    episodes.reverse();
    episodes
}

fn fetch_title_id(path: &str, request: &Value) -> Option<String> {
    let base = base_url(request);
    let body = get_or_fixture(&absolute_url(&base, path), DETAILS_FIXTURE, &base);
    body.split("id:")
        .nth(1)
        .and_then(|tail| tail.split(['\'', '"']).nth(1))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn encrypt(text: &str) -> Option<String> {
    let target = format!("{ENC_MOVIES_URL}?text={}", url::query_escape(text));
    HttpClient::browser()
        .get(target)
        .xhr()
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<ResultResponse>(&body).ok())
        .map(|response| response.result)
}

fn decrypt_link(text: &str) -> Option<String> {
    let target = format!("{DEC_MOVIES_URL}?text={}", url::query_escape(text));
    HttpClient::browser()
        .get(target)
        .xhr()
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<DecryptedIframeResponse>(&body).ok())
        .map(|response| response.result.url)
}

fn rapidshare_streams(url: &str, prefix: &str, request: &Value) -> Vec<VideoStream> {
    let target = if path_segment(url, 0).as_deref() == Some("iframe") {
        unwrap_iframe_url(url).unwrap_or_else(|| url.to_string())
    } else {
        url.to_string()
    };
    let Some(token) = path_segment(&target, usize::MAX) else {
        return vec![external_stream(&target, prefix, BASE_URL, request)];
    };
    let Some(base) = origin_from_url(&target) else {
        return vec![external_stream(&target, prefix, BASE_URL, request)];
    };
    let media_url = format!("{base}/media/{token}");
    let encrypted = HttpClient::browser()
        .with_header("User-Agent", USER_AGENT)
        .with_referer(&target)
        .get(&media_url)
        .xhr()
        .referer(&target)
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<EncryptedRapidResponse>(&body).ok())
        .map(|response| response.result);
    let Some(encrypted) = encrypted else {
        return Vec::new();
    };
    let body = json!({ "text": encrypted, "agent": USER_AGENT }).to_string();
    let decrypted = HttpClient::browser()
        .with_header("User-Agent", USER_AGENT)
        .with_referer(&target)
        .with_origin(origin_from_url(BASE_URL).unwrap_or_else(|| BASE_URL.to_string()))
        .post(DEC_RAPID_URL)
        .json(body)
        .referer(BASE_URL)
        .send_text()
        .ok()
        .and_then(|text| serde_json::from_str::<RapidDecryptResponse>(&text).ok())
        .map(|response| response.result);
    let Some(decrypted) = decrypted else {
        return Vec::new();
    };
    let subtitles = subtitle_tracks(
        subtitle_query(&target)
            .and_then(|sub_url| fetch_subtitles(&sub_url, &base))
            .unwrap_or(decrypted.tracks),
        &base,
        &pref(request, "pref_sub_lang_key", "English"),
    );
    let mut streams = Vec::new();
    for source in decrypted.sources {
        if source.file.contains(".m3u8") {
            streams.extend(expand_hls(&source.file, prefix, &base, &subtitles, request));
        }
    }
    streams
}

fn unwrap_iframe_url(target: &str) -> Option<String> {
    let body = HttpClient::browser()
        .with_header("User-Agent", USER_AGENT)
        .with_referer(target)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .referer(target)
        .send_text()
        .ok()
        .or_else(|| {
            webview::extract_text(
                webview::ExtractRequest::new(
                    target,
                    "(() => { const iframe = document.querySelector('iframe[src]'); return iframe ? iframe.src : ''; })()",
                )
                .user_agent(USER_AGENT)
                .wait_for_selector("iframe[src]")
                .timeout_ms(15_000),
            )
            .ok()
        })?;
    iframe_src(&body, target).or_else(|| {
        body.trim()
            .starts_with("http")
            .then(|| body.trim().to_string())
    })
}

fn iframe_src(body: &str, base: &str) -> Option<String> {
    body.split("<iframe")
        .skip(1)
        .filter_map(|chunk| attr_from_html(chunk, "src"))
        .find(|src| src.contains("/e/") || src.to_ascii_lowercase().contains("rapidshare"))
        .map(|src| {
            absolute_url(
                &origin_from_url(base).unwrap_or_else(|| base.to_string()),
                &src,
            )
        })
}

fn fetch_subtitles(url: &str, base: &str) -> Option<Vec<RapidShareTrack>> {
    HttpClient::browser()
        .with_header("User-Agent", USER_AGENT)
        .with_referer(format!("{base}/"))
        .with_origin(base)
        .get(url)
        .header("Accept", "*/*")
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<Vec<RapidShareTrack>>(&body).ok())
}

fn subtitle_tracks(
    mut tracks: Vec<RapidShareTrack>,
    base: &str,
    preferred_lang: &str,
) -> Vec<SubtitleTrack> {
    tracks.sort_by_key(|track| {
        !track
            .label
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&preferred_lang.to_ascii_lowercase())
    });
    tracks
        .into_iter()
        .filter(|track| track.kind == "captions" && !track.file.is_empty())
        .filter_map(|track| {
            let label = track.label?;
            Some(SubtitleTrack {
                url: absolute_url(base, &track.file),
                language: language_code(&label),
                label: Some(label.clone()),
                format: subtitle_format(&track.file),
                headers: referer_headers(&format!("{base}/")),
                is_default: label.eq_ignore_ascii_case(preferred_lang),
                ..SubtitleTrack::default()
            })
        })
        .collect()
}

fn expand_hls(
    target: &str,
    prefix: &str,
    referer: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> Vec<VideoStream> {
    let body = HttpClient::browser()
        .get(target)
        .headers(referer_headers(&format!("{referer}/")))
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(
            target, prefix, "auto", referer, subtitles, request,
        )];
    }
    let mut streams = Vec::new();
    let mut pending_quality = None::<String>;
    for line in body.lines().map(str::trim) {
        if let Some(info) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_quality = quality_from_stream_info(info);
        } else if !line.is_empty() && !line.starts_with('#') {
            let stream_url = absolute_media_url(target, line);
            let quality = pending_quality.take().unwrap_or_else(|| "auto".to_string());
            streams.push(media_stream(
                &stream_url,
                prefix,
                &quality,
                referer,
                subtitles,
                request,
            ));
        }
    }
    streams
}

fn media_stream(
    target: &str,
    prefix: &str,
    quality: &str,
    referer: &str,
    subtitles: &[SubtitleTrack],
    request: &Value,
) -> VideoStream {
    let is_hls = target.contains(".m3u8");
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{prefix} - {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(&format!("{referer}/")),
        subtitles: subtitles.to_vec(),
        preferred: is_preferred(quality, prefix, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, prefix: &str, referer: &str, request: &Value) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{prefix} - External")),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        preferred: is_preferred("", prefix, request),
        initialized: true,
        ..VideoStream::default()
    }
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
        (i32::from(stream.preferred), quality_score)
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = is_preferred(
            stream.quality.as_deref().unwrap_or_default(),
            stream.name.as_deref().unwrap_or_default(),
            request,
        );
    }
}

fn is_preferred(quality: &str, server: &str, request: &Value) -> bool {
    let pref_quality = pref(request, "pref_quality_key", "1080p");
    let pref_server = pref(request, "pref_server_key", "Server 1");
    quality.contains(&pref_quality)
        || server
            .to_ascii_lowercase()
            .contains(&pref_server.to_ascii_lowercase())
}

fn search_url(base: &str, query: &str, request: &Value) -> String {
    let mut pairs = vec![
        ("keyword".to_string(), query.to_string()),
        ("page".to_string(), page(request).to_string()),
    ];
    push_indexed(&mut pairs, "type", &filter_values(request, "type"));
    push_indexed(&mut pairs, "year", &filter_values(request, "year"));
    push_indexed(&mut pairs, "quality", &filter_values(request, "quality"));
    let genres = filter_values(request, "genre");
    push_indexed(&mut pairs, "genre", &genres);
    if !genres.is_empty() {
        pairs.push((
            "genre_mode".to_string(),
            filter_text(request, "genre_mode", "and"),
        ));
    }
    let countries = filter_values(request, "country");
    push_indexed(&mut pairs, "country", &countries);
    if !countries.is_empty() {
        pairs.push((
            "country_mode".to_string(),
            filter_text(request, "country_mode", "or"),
        ));
    }
    let sort = filter_text(request, "sort", "");
    if !sort.is_empty() {
        pairs.push(("sort".to_string(), sort));
    }
    let query = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}/browser?{query}")
}

fn push_indexed(pairs: &mut Vec<(String, String)>, key: &str, values: &[String]) {
    for (index, value) in values.iter().enumerate() {
        pairs.push((format!("{key}[{}]", index + 1), value.clone()));
    }
}

fn filter_values(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(value_string).collect())
        .unwrap_or_default()
}

fn filter_text(request: &Value, key: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(value_string)
        .unwrap_or_else(|| default.to_string())
}

fn value_string(value: &Value) -> Option<String> {
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn enabled_servers(request: &Value) -> Vec<String> {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("pref_hoster_key"))
        .or_else(|| request.get("pref_hoster_key"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| SERVERS.iter().map(ToString::to_string).collect())
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn base_url(request: &Value) -> String {
    pref(request, "pref_domain_key", BASE_URL)
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

fn path_from_url(input: &str, base: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with(base) || trimmed.starts_with("/watch/") {
        Some(path_key(trimmed))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.find('/').map(|index| &rest[index..]))
        .unwrap_or(input)
        .split(['#', '?'])
        .next()
        .unwrap_or(input);
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

fn origin_from_url(input: &str) -> Option<String> {
    let (scheme, rest) = input.split_once("://")?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}://{host}"))
}

fn path_segment(input: &str, index: usize) -> Option<String> {
    let path = input.split("://").nth(1)?.split_once('/')?.1;
    let segments = path
        .split(['?', '#'])
        .next()?
        .split('/')
        .collect::<Vec<_>>();
    if index == usize::MAX {
        return segments
            .last()
            .filter(|value| !value.is_empty())
            .map(|value| (*value).to_string());
    }
    segments
        .get(index)
        .filter(|value| !value.is_empty())
        .map(|value| (*value).to_string())
}

fn subtitle_query(input: &str) -> Option<String> {
    let query = input.split('?').nth(1)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == "sub.list" {
            return Some(value.to_string());
        }
    }
    None
}

fn absolute_media_url(base: &str, input: &str) -> String {
    if input.starts_with("http") {
        return input.to_string();
    }
    let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        input.trim_start_matches('/')
    )
}

fn quality_from_stream_info(info: &str) -> Option<String> {
    info.split("RESOLUTION=")
        .nth(1)
        .and_then(|part| part.split('x').nth(1))
        .and_then(|part| part.split([',', '\n', '\r']).next())
        .filter(|height| !height.is_empty())
        .map(|height| format!("{height}p"))
}

fn subtitle_format(url: &str) -> Option<String> {
    if url.to_ascii_lowercase().ends_with(".srt") {
        Some("srt".to_string())
    } else {
        Some("vtt".to_string())
    }
}

fn language_code(label: &str) -> Option<String> {
    let lower = label.to_ascii_lowercase();
    let code = match lower.as_str() {
        value if value.contains("english") => "en",
        value if value.contains("arabic") => "ar",
        value if value.contains("chinese") => "zh",
        value if value.contains("french") => "fr",
        value if value.contains("german") => "de",
        value if value.contains("indonesian") => "id",
        value if value.contains("italian") => "it",
        value if value.contains("japanese") => "ja",
        value if value.contains("korean") => "ko",
        value if value.contains("portuguese") => "pt",
        value if value.contains("russian") => "ru",
        value if value.contains("spanish") => "es",
        value if value.contains("turkish") => "tr",
        value if value.contains("vietnamese") => "vi",
        _ => return None,
    };
    Some(code.to_string())
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn with_listing(request: &Value, value: &str) -> Value {
    let mut request = request.clone();
    if let Some(map) = request.as_object_mut() {
        map.insert("listing".to_string(), Value::String(value.to_string()));
    }
    request
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn fallback_item(path: &str, base: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(base, path)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("123Cine")
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

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

fn fancy_score(score: &str) -> Option<String> {
    let score = score.trim().parse::<f32>().ok()?;
    if score <= 0.0 {
        return None;
    }
    let stars = ((score / 2.0).round() as usize).min(5);
    Some(format!(
        "{}{} {}",
        "*".repeat(stars),
        "-".repeat(5 - stars),
        trim_float(score)
    ))
}

fn trim_float(value: f32) -> String {
    let text = format!("{value:.1}");
    text.trim_end_matches(".0").to_string()
}

fn push_line(out: &mut String, value: &str) {
    if !out.trim().is_empty() {
        out.push('\n');
    }
    out.push_str(value);
}

fn push_meta(out: &mut String, label: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    push_line(out, &format!("{label}: {}", value.trim()));
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
        .and_then(|element| attr(&element, name))
}

fn select_text(doc: &Html, query: &str) -> Option<String> {
    select_all(doc, query)
        .next()
        .map(|element| collect_text(&element))
}

fn attr_of_first(element: ElementRef<'_>, query: &str, name: &str) -> Option<String> {
    select_all_in(element, query)
        .next()
        .and_then(|element| attr(&element, name))
}

fn text_of_first(element: ElementRef<'_>, query: &str) -> Option<String> {
    select_all_in(element, query)
        .next()
        .map(|element| collect_text(&element))
        .filter(|value| !value.is_empty())
}

fn attr(element: &ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn attr_from_html(chunk: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{name}={quote}");
        if let Some(value) = chunk
            .split(&pattern)
            .nth(1)
            .and_then(|tail| tail.split(quote).next())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn collect_text(element: &ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_texts_in(element: ElementRef<'_>, query: &str) -> Vec<String> {
    select_all_in(element, query)
        .map(|node| collect_text(&node))
        .filter(|value| !value.is_empty())
        .collect()
}

fn meta_value(meta_foot: Option<ElementRef<'_>>, label: &str) -> String {
    let Some(meta) = meta_foot else {
        return String::new();
    };
    select_all_in(meta, "div")
        .find_map(|node| {
            let text = collect_text(&node);
            text.strip_prefix(label)
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_default()
}

fn side_meta_value(doc: &Html, label: &str) -> String {
    let heading_selector = selector(".mini-meta-line .mini-meta h2, .mini-meta h2");
    for heading in doc.select(&heading_selector) {
        if !collect_text(&heading).contains(label) {
            continue;
        }
        if let Some(parent) = heading.parent().and_then(ElementRef::wrap) {
            let values = collect_texts_in(parent, "div a");
            if !values.is_empty() {
                return values.join(", ");
            }
        }
    }
    String::new()
}

fn highlight_values(doc: &Html) -> Vec<String> {
    let heading_selector = selector(".mini-meta h2");
    for heading in doc.select(&heading_selector) {
        if !collect_text(&heading).contains("Highlight") {
            continue;
        }
        if let Some(parent) = heading.parent().and_then(ElementRef::wrap) {
            return collect_texts_in(parent, "div span");
        }
    }
    Vec::new()
}

#[derive(Default, Deserialize)]
struct ResultResponse {
    #[serde(default)]
    result: String,
}

#[derive(Default, Deserialize)]
struct DecryptedIframeResponse {
    #[serde(default)]
    result: DecryptedIframeResult,
}

#[derive(Default, Deserialize)]
struct DecryptedIframeResult {
    #[serde(default)]
    url: String,
}

#[derive(Default, Deserialize)]
struct EpisodesResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    result: EpisodesResult,
}

#[derive(Default, Deserialize)]
struct EpisodesResult {
    #[serde(default)]
    title: TitleDto,
    #[serde(default)]
    seasons: Vec<SeasonDto>,
}

#[derive(Default, Deserialize)]
struct TitleDto {
    #[serde(default, rename = "type")]
    media_type: String,
}

#[derive(Default, Deserialize)]
struct SeasonDto {
    #[serde(default)]
    number: u32,
    #[serde(default)]
    episodes: Vec<EpisodeDto>,
}

#[derive(Default, Deserialize)]
struct EpisodeDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    number: f32,
    #[serde(default)]
    detail_name: Option<String>,
}

#[derive(Default, Deserialize)]
struct EpisodeServersResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    result: EpisodeServersResult,
}

#[derive(Default, Deserialize)]
struct EpisodeServersResult {
    #[serde(default)]
    links: Vec<LinkDto>,
}

#[derive(Default, Deserialize)]
struct LinkDto {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct LinkResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    result: String,
}

#[derive(Default, Deserialize)]
struct EncryptedRapidResponse {
    #[serde(default)]
    result: String,
}

#[derive(Default, Deserialize)]
struct RapidDecryptResponse {
    #[serde(default)]
    result: RapidShareResult,
}

#[derive(Default, Deserialize)]
struct RapidShareResult {
    #[serde(default)]
    sources: Vec<RapidShareSource>,
    #[serde(default)]
    tracks: Vec<RapidShareTrack>,
}

#[derive(Deserialize)]
struct RapidShareSource {
    file: String,
}

#[derive(Deserialize)]
struct RapidShareTrack {
    #[serde(default)]
    file: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    kind: String,
}

const LIST_FIXTURE: &str = r#"
<html><body>
<div class="item"><div class="inner"><a class="poster" href="/watch/sample-movie"><img src="/poster.jpg"></a><div class="detail"><div class="title">Sample Movie</div></div></div></div>
</body></html>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><body>
<div x-data="Watch({ id: 'sample-title' })"></div>
<div class="head-movie-wrapper"><h1 class="title">Sample Movie</h1><div class="metadata"><span class="dot">Movie</span><span class="dot">90 min</span></div></div>
<div class="detail-start"><div class="poster"><img src="/poster.jpg"></div></div>
<div class="movie-info"><div class="desc">A fixture movie used for smoke tests.</div></div>
<div class="mini-meta-foot"><a href="/genre/14">Action</a><div>Released: <span>2026</span></div><div>Quality: <span>HD</span></div></div>
<div class="mini-meta"><h2>Highlight</h2><div><span>IMDb 7.2</span><span>PG-13</span></div></div>
</body></html>
"#;

const EPISODES_FIXTURE: &str = r#"{"status":"ok","result":{"title":{"type":"movie"},"seasons":[{"number":1,"episodes":[{"id":"sample-episode","number":1,"detail_name":"Movie"}]}]}}"#;
const SERVERS_FIXTURE: &str =
    r#"{"status":"ok","result":{"links":[{"id":"sample-link","name":"Server 1"}]}}"#;
const LINK_FIXTURE: &str = r#"{"status":"error","result":""}"#;

export_video_source!(SOURCE);
