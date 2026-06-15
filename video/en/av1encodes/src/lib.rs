use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, TorrentInfo, UrlResolveResult,
    VideoEpisode, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{SearchRequest, http::HttpClient},
    video::referer_headers,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Av1Encodes = Av1Encodes;
const DEFAULT_BASE_URL: &str = "https://av1encodes.com";
const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

struct Av1Encodes;

impl VideoSource for Av1Encodes {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let listing = listing(&request);
        let target = if listing == "latest" {
            base.clone()
        } else {
            format!("{base}/stats#top-downloads")
        };
        let body = get_or_fixture(&base, &target, LIST_FIXTURE);
        let entries = if listing == "latest" {
            parse_card_list(&base, &body).entries
        } else {
            parse_stats_page(&base, &body)
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&base, &path)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let target = format!(
                "{base}/search?q={}&page={}",
                manatan_shared::sdk::http::url_encode(query),
                page(&request)
            );
            let body = get_or_fixture(&base, &target, SEARCH_FIXTURE);
            return Ok(parse_card_list(&base, &body));
        }

        let mut target = format!("{base}/anime?page={}", page(&request));
        if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            target.push_str("&genres=");
            target.push_str(&manatan_shared::sdk::http::url_encode(&genre));
        }
        if let Some(sort) = filter(&request, "sort").filter(|value| !value.is_empty()) {
            target.push_str("&sort=");
            target.push_str(&manatan_shared::sdk::http::url_encode(&sort));
        }
        if let Some(kind) = filter(&request, "type").filter(|value| !value.is_empty()) {
            target.push_str("&type=");
            target.push_str(&manatan_shared::sdk::http::url_encode(&kind));
        }
        let body = get_or_fixture(&base, &target, SEARCH_FIXTURE);
        Ok(parse_anime_list_page(&base, &body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/anime/sample-anime".to_string());
        Ok(fetch_details(&base, &path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = base_url(&request);
        let path =
            request_key(&request, "item").unwrap_or_else(|| "/anime/sample-anime".to_string());
        let body = get_or_fixture(&base, &absolute_url(&base, &path), DETAILS_FIXTURE);
        Ok(parse_episodes(&base, &path, &body, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = base_url(&request);
        let episode = request_key(&request, "episode").unwrap_or_else(|| {
            "/download/sample-anime/1/1920%20x%201080/%5BE01%5D%20Sample%20%5B1080p%5D.mkv"
                .to_string()
        });
        let mut streams = resolve_streams(&base, &episode, &request);
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
        let base = base_url(&request);
        Ok(request_key(&request, "item").map(|path| absolute_url(&base, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(request_key(&request, "episode").map(|path| absolute_url(&base, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url(input) {
            let base = base_url(&request);
            if path.starts_with("/download/") {
                return Ok(Some(UrlResolveResult {
                    episode: Some(json!({
                        "key": path,
                        "title": title_from_path(&path),
                        "url": absolute_url(&base, &path),
                        "language": "en"
                    })),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", DESKTOP_UA)
        .with_header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .with_header(
            "Sec-Ch-Ua",
            "\"Chromium\";v=\"124\", \"Google Chrome\";v=\"124\"",
        )
        .with_header("Sec-Ch-Ua-Mobile", "?0")
        .with_header("Sec-Ch-Ua-Platform", "\"Windows\"")
        .with_referer(base)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(base: &str, target: &str, fixture: &str) -> String {
    match client(base)
        .get(target)
        .browser_document()
        .referer(base)
        .send_text()
    {
        Ok(body) => body,
        Err(error) if format!("{error:?}").contains("live HTTP is disabled during smoke tests") => {
            fixture.to_string()
        }
        Err(_) => String::new(),
    }
}

fn fetch_details(base: &str, path: &str) -> CatalogItem {
    let body = get_or_fixture(base, &absolute_url(base, path), DETAILS_FIXTURE);
    parse_details(base, &body, path).unwrap_or_else(|| fallback_item(base, path))
}

fn parse_stats_page(base: &str, body: &str) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut entries = Vec::new();
    for element in select_all(
        &doc,
        "a[href*='/anime/'], div[class*='card'], div[class*='item'], li",
    ) {
        let text = collect_text(&element);
        if !(text.contains("[S") || (10..=200).contains(&text.len())) {
            continue;
        }
        if let Some(link) = href_from(&element, "a[href*='/anime/']") {
            let path = path_key(&link);
            if path.starts_with("/anime/") && path != "/anime/" {
                entries.push(card_item(
                    base,
                    &path,
                    extract_clean_title(&text),
                    list_image_url(base, &element),
                ));
            }
        }
    }
    if entries.is_empty() {
        let text = doc.root_element().text().collect::<Vec<_>>().join(" ");
        let re = Regex::new(r"\[S\d{1,2}(?:-E\d+)?]\s*([^\[]+?)\s*\[").expect("valid regex");
        for cap in re.captures_iter(&text).take(20) {
            let title = extract_clean_title(cap.get(1).map(|m| m.as_str()).unwrap_or_default());
            let slug = slugify(&title);
            if slug.len() >= 3 {
                entries.push(card_item(base, &format!("/anime/{slug}"), title, None));
            }
        }
    }
    dedupe_items(entries)
}

fn parse_card_list(base: &str, body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut entries = Vec::new();
    for card in select_all(
        &doc,
        "article.anime-card, article[class*='card'], article[class*='anime']",
    ) {
        let Some(href) = href_from(&card, "h3 > a, h4 > a, .card-body a, a[href*='/anime/']")
        else {
            continue;
        };
        let path = path_key(&href);
        if !path.starts_with("/anime/") || path == "/anime/" {
            continue;
        }
        let title = text(&card, "h3, h4")
            .or_else(|| text(&card, "a[href*='/anime/']"))
            .unwrap_or_else(|| title_from_path(&path));
        entries.push(card_item(base, &path, title, list_image_url(base, &card)));
    }

    if entries.is_empty() {
        for heading in select_all(&doc, "h3") {
            let block = heading
                .parent()
                .and_then(ElementRef::wrap)
                .unwrap_or(heading);
            let Some(href) = href_from(&block, "a[href*='/anime/']") else {
                continue;
            };
            let path = path_key(&href);
            if !path.starts_with("/anime/") || path == "/anime/" {
                continue;
            }
            entries.push(card_item(
                base,
                &path,
                collect_text(&heading),
                list_image_url(base, &block),
            ));
        }
    }

    let has_next_page =
        select_all(&doc, ".pagination a, nav.pagination a, a[rel='next']").any(|a| {
            attr_value(&a, "rel").as_deref() == Some("next") || collect_text(&a).contains("Next")
        });
    Paged {
        entries: dedupe_items(entries),
        has_next_page,
    }
}

fn parse_anime_list_page(base: &str, body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let entries = select_all(&doc, "li > a[href*='/anime/']")
        .filter_map(|link| {
            let href = link.value().attr("href")?;
            let path = path_key(href);
            if !path.starts_with("/anime/") || path == "/anime/" {
                return None;
            }
            let title = collect_text(&link);
            (!title.is_empty()).then(|| card_item(base, &path, title, None))
        })
        .collect::<Vec<_>>();
    let has_next_page =
        select_all(&doc, "a[rel='next'], .pagination .next, .pagination a").any(|a| {
            attr_value(&a, "rel").as_deref() == Some("next") || collect_text(&a).contains("Next")
        });
    Paged {
        entries: dedupe_items(entries),
        has_next_page,
    }
}

fn parse_details(base: &str, body: &str, path: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    let title = select_text(
        &doc,
        ".anime-hero h1, h1.anime-title, [class*='anime-hero'] h1, [class*='detail'] h1, main h1, h1",
    )
    .unwrap_or_else(|| title_from_path(path));
    let cover = select_attr(
        &doc,
        "img.anime-poster, img.poster, .anime-hero img, [class*='poster'] img, [class*='hero'] img, main img",
        "data-src",
    )
    .or_else(|| {
        select_attr(
            &doc,
            "img.anime-poster, img.poster, .anime-hero img, [class*='poster'] img, [class*='hero'] img, main img",
            "src",
        )
    })
    .or_else(|| select_attr(&doc, "meta[property='og:image']", "content"))
    .map(|image| absolute_url(base, &image));
    let tags = select_all(
        &doc,
        ".genre-tag, .tag, a[href*='/genre/'], a[href*='/tag/'], [class*='genre'] a",
    )
    .map(|tag| collect_text(&tag))
    .filter(|tag| !tag.is_empty())
    .collect();
    let status = if select_all(&doc, "[class*='airing'], .status-airing, .airing-badge")
        .next()
        .is_some()
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Completed
    };
    Some(CatalogItem {
        key: path_key(path),
        title,
        cover,
        url: Some(absolute_url(base, path)),
        description: select_text(
            &doc,
            ".anime-synopsis, .synopsis, .description, [class*='synopsis'], [class*='description'], [class*='overview'], .desc",
        ),
        tags,
        authors: select_text(&doc, ".studio, .studio-name, [class*='studio']")
            .into_iter()
            .collect(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(
    base: &str,
    item_path: &str,
    details_body: &str,
    request: &Value,
) -> Vec<VideoEpisode> {
    let slug = item_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample-anime");
    let doc = Html::parse_document(details_body);
    let mut seasons = select_all(
        &doc,
        ".season-tab[data-season], .season-option[data-season], [data-season]",
    )
    .filter_map(|el| attr_value(&el, "data-season"))
    .filter(|season| !season.is_empty())
    .collect::<Vec<_>>();
    seasons.sort();
    seasons.dedup();
    if seasons.is_empty() {
        seasons.push("1".to_string());
    }

    let encoded_quality = path_encode(&preferred_quality(request));
    let mut out = Vec::new();
    for season in seasons {
        let target = format!("{base}/episodes/{slug}/{season}/{encoded_quality}");
        let body = get_or_fixture(base, &target, EPISODES_FIXTURE);
        let mut season_entries = parse_episode_json(base, slug, &season, &body)
            .unwrap_or_else(|| parse_episode_html(base, slug, &season, &encoded_quality, &body));
        out.append(&mut season_entries);
    }
    out.sort_by(|a, b| {
        b.season_number
            .partial_cmp(&a.season_number)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.episode_number
                    .partial_cmp(&a.episode_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    out
}

fn parse_episode_json(
    base: &str,
    slug: &str,
    season: &str,
    body: &str,
) -> Option<Vec<VideoEpisode>> {
    let items: Vec<EpisodeItem> = serde_json::from_str(body).ok()?;
    Some(
        items
            .into_iter()
            .filter(|item| !item.href.is_empty())
            .map(|item| {
                let filename = path_decode(
                    item.href
                        .split('?')
                        .next()
                        .unwrap_or(&item.href)
                        .rsplit('/')
                        .next()
                        .unwrap_or(""),
                );
                let title = if item.label.is_empty() {
                    build_episode_label(&filename, season)
                } else {
                    item.label
                };
                let href = if item.href.starts_with("/download/") {
                    item.href
                } else {
                    format!("/download/{slug}/{season}/{}", path_encode(&filename))
                };
                episode_item(base, &href, season, item.num as f32, title, filename)
            })
            .collect(),
    )
}

fn parse_episode_html(
    base: &str,
    slug: &str,
    season: &str,
    encoded_quality: &str,
    body: &str,
) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let mut out = select_all(&doc, "a[href*='/download/']")
        .filter_map(|link| {
            let href = link.value().attr("href")?;
            let path = path_key_keep_query(href);
            let filename = path_decode(
                path.split('?')
                    .next()
                    .unwrap_or(&path)
                    .rsplit('/')
                    .next()
                    .unwrap_or(""),
            );
            let number = episode_number(&filename);
            Some(episode_item(
                base,
                &path,
                season,
                number,
                build_episode_label(&filename, season),
                filename,
            ))
        })
        .collect::<Vec<_>>();
    if out.is_empty() {
        for filename in extract_filenames(body) {
            let href = format!(
                "/download/{slug}/{season}/{encoded_quality}/{}",
                path_encode(&filename)
            );
            out.push(episode_item(
                base,
                &href,
                season,
                episode_number(&filename),
                build_episode_label(&filename, season),
                filename,
            ));
        }
    }
    out
}

fn episode_item(
    base: &str,
    href: &str,
    season: &str,
    episode_number: f32,
    title: String,
    filename: String,
) -> VideoEpisode {
    VideoEpisode {
        key: path_key_keep_query(href),
        title: Some(title),
        episode_number: Some(episode_number),
        season_number: season.parse::<f32>().ok(),
        url: Some(absolute_url(base, href)),
        variant: audio_tag(&filename),
        language: Some("en".to_string()),
        labels: quality_label(&filename).into_iter().collect(),
        ..VideoEpisode::default()
    }
}

fn resolve_streams(base: &str, episode_path: &str, request: &Value) -> Vec<VideoStream> {
    let path = path_key_keep_query(episode_path);
    let encoded_filename = path
        .split('?')
        .next()
        .unwrap_or(&path)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let filename = path_decode(encoded_filename);
    let download_url = absolute_url(base, &path);
    let page = client(base)
        .get(&download_url)
        .browser_document()
        .referer(format!("{base}/"))
        .send_text();
    let Ok(page_html) = page else {
        return fallback_stream(base, &path, &filename);
    };
    let token_re = Regex::new(r#"['"](A{4,}[A-Za-z0-9_\-]{10,})['"]"#).expect("valid regex");
    let Some(token) = token_re
        .captures(&page_html)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
    else {
        return fallback_stream(base, &path, &filename);
    };

    let ddl_url = format!("{base}/get_ddl/{encoded_filename}");
    let ddl_raw = client(base)
        .get(ddl_url)
        .xhr()
        .header("X-Ddl-Token", token)
        .referer(&download_url)
        .send_text();
    let Ok(ddl_raw) = ddl_raw else {
        return fallback_stream(base, &path, &filename);
    };
    let Ok(ddl) = serde_json::from_str::<DdlResponse>(&ddl_raw) else {
        return fallback_stream(base, &path, &filename);
    };
    if !ddl.success {
        return fallback_stream(base, &path, &filename);
    }
    streams_from_ddl(base, &download_url, &filename, ddl, request)
        .into_iter()
        .collect::<Vec<_>>()
        .tap_empty(|| fallback_stream(base, &path, &filename))
}

fn streams_from_ddl(
    base: &str,
    download_url: &str,
    filename: &str,
    ddl: DdlResponse,
    request: &Value,
) -> Vec<VideoStream> {
    let mut out = Vec::new();
    let quality = stream_quality(filename, ddl.file_size.as_deref(), request);
    if let Some(watch_url) = resolve_redirect(base, download_url, ddl.watch_link.as_deref()) {
        if watch_url.contains("/watch/") {
            let mpd = format!(
                "{}/manifest.mpd",
                watch_url.replace("/watch/", "/dash/").trim_end_matches('/')
            );
            out.push(stream(
                mpd,
                format!("{quality} · DASH"),
                "dash",
                VideoStreamKind::Dash,
                base,
                request,
            ));
        }
    }
    if let Some(stream_url) = resolve_redirect(base, download_url, ddl.stream_link.as_deref()) {
        out.push(stream(
            stream_url,
            format!("{quality} · Stream"),
            "mp4",
            VideoStreamKind::Direct,
            base,
            request,
        ));
    }
    if let Some(dl_url) = resolve_redirect(base, download_url, ddl.download_link.as_deref()) {
        out.push(stream(
            dl_url,
            format!("{quality} · Direct DL"),
            "mkv",
            VideoStreamKind::Direct,
            base,
            request,
        ));
    }
    if show_torrent(request) {
        if let Some(torrent_url) = resolve_redirect(base, download_url, ddl.torrent_link.as_deref())
        {
            let mut item = stream(
                torrent_url.clone(),
                format!("{quality} · Torrent"),
                "torrent",
                VideoStreamKind::Torrent,
                base,
                request,
            );
            item.torrent = Some(TorrentInfo {
                torrent_url: Some(torrent_url),
                file_name: Some(filename.to_string()),
                ..TorrentInfo::default()
            });
            out.push(item);
        }
    }
    out
}

fn resolve_redirect(base: &str, referer: &str, path: Option<&str>) -> Option<String> {
    let path = path.filter(|value| !value.trim().is_empty())?;
    let target = absolute_url(base, path);
    client(base)
        .get(&target)
        .referer(referer)
        .send()
        .ok()
        .map(|response| response.final_url)
        .filter(|url| !url.is_empty())
        .or(Some(target))
}

fn fallback_stream(base: &str, episode_path: &str, filename: &str) -> Vec<VideoStream> {
    vec![stream(
        absolute_url(base, episode_path),
        format!("{} · Direct DL", stream_quality_from_filename(filename)),
        "mkv",
        VideoStreamKind::Direct,
        base,
        &json!({}),
    )]
}

fn stream(
    url: String,
    name: String,
    format: &str,
    kind: VideoStreamKind,
    base: &str,
    request: &Value,
) -> VideoStream {
    let is_dash = matches!(kind, VideoStreamKind::Dash);
    let is_hls = matches!(kind, VideoStreamKind::Hls);
    let quality = resolution_from_label(&name).unwrap_or_else(|| name.clone());
    VideoStream {
        url,
        name: Some(name.clone()),
        quality: Some(quality.clone()),
        resolution: Some(quality),
        format: Some(format.to_string()),
        video_codec: Some("AV1".to_string()),
        is_dash,
        is_hls,
        stream_kind: Some(kind),
        headers: referer_headers(base),
        preferred: is_preferred(&name, request),
        initialized: true,
        ..VideoStream::default()
    }
}

fn card_item(base: &str, path: &str, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title,
        cover: cover.map(|image| absolute_url(base, &image)),
        url: Some(absolute_url(base, path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fallback_item(base: &str, path: &str) -> CatalogItem {
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(base, path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn dedupe_items(items: Vec<CatalogItem>) -> Vec<CatalogItem> {
    let mut seen = Vec::<String>::new();
    items
        .into_iter()
        .filter(|item| {
            if seen.iter().any(|key| key == &item.key) {
                false
            } else {
                seen.push(item.key.clone());
                true
            }
        })
        .collect()
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
        .filter(|text| !text.is_empty())
}

fn select_attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    select_all(doc, selector)
        .next()
        .and_then(|element| attr_value(&element, name))
}

fn text(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    select_all_from(element, selector)
        .next()
        .map(|value| collect_text(&value))
        .filter(|value| !value.is_empty())
}

fn href_from(element: &ElementRef<'_>, selector: &str) -> Option<String> {
    if element.value().name() == "a" {
        if let Some(href) = element.value().attr("href") {
            return Some(href.to_string());
        }
    }
    select_all_from(element, selector)
        .next()
        .and_then(|value| value.value().attr("href").map(ToString::to_string))
}

fn attr_value(element: &ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(ToString::to_string)
        .filter(|value| !value.is_empty())
}

fn collect_text(element: &ElementRef<'_>) -> String {
    html::html_unescape(&element.text().collect::<Vec<_>>().join(" "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn list_image_url(base: &str, element: &ElementRef<'_>) -> Option<String> {
    select_all_from(element, "img")
        .find_map(|img| {
            attr_value(&img, "data-src")
                .or_else(|| attr_value(&img, "data-lazy-src"))
                .or_else(|| attr_value(&img, "src"))
        })
        .or_else(|| background_url(element))
        .map(|image| absolute_url(base, &image))
}

fn background_url(element: &ElementRef<'_>) -> Option<String> {
    let style = element.value().attr("style")?;
    if !style.to_ascii_lowercase().contains("background") {
        return None;
    }
    Regex::new(r#"url\(['"]?(.*?)['"]?\)"#)
        .expect("valid regex")
        .captures(style)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
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
        .map(path_key_keep_query)
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(DEFAULT_BASE_URL)
        || input.starts_with("https://av1please.com")
        || input.starts_with("/anime/")
        || input.starts_with("/download/"))
    .then(|| path_key_keep_query(input))
}

fn path_key(input: &str) -> String {
    path_key_keep_query(input)
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string()
}

fn path_key_keep_query(input: &str) -> String {
    let value = if input.starts_with(DEFAULT_BASE_URL) {
        input.trim_start_matches(DEFAULT_BASE_URL).to_string()
    } else if input.starts_with("https://av1please.com") {
        input
            .trim_start_matches("https://av1please.com")
            .to_string()
    } else if input.starts_with("http") {
        let rest = input.split("://").nth(1).unwrap_or(input);
        rest.split_once('/')
            .map(|(_, path)| format!("/{path}"))
            .unwrap_or_else(|| "/".to_string())
    } else {
        input.to_string()
    };
    format!("/{}", value.trim_start_matches('/'))
        .split('#')
        .next()
        .unwrap_or("/")
        .to_string()
}

fn absolute_url(base: &str, input: &str) -> String {
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

fn title_from_path(path: &str) -> String {
    path.split('?')
        .next()
        .unwrap_or(path)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("AV1Encodes")
        .replace(['-', '_', '+'], " ")
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

fn extract_clean_title(raw: &str) -> String {
    let mut value = raw.to_string();
    for pattern in [
        r"(?i)\s*·\s*\d+\s*downloads?.*",
        r"^\[[a-zA-Z0-9_\-]+]\s*",
        r"(?i)\s*\[\d{3,4}p].*",
        r"(?i)\.(mkv|mp4)$",
    ] {
        value = Regex::new(pattern)
            .expect("valid regex")
            .replace_all(&value, "")
            .to_string();
    }
    value.trim().to_string()
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn extract_filenames(body: &str) -> Vec<String> {
    let re = Regex::new(r"(?i)([a-zA-Z0-9_ \-\[\]().%]+?\.(?:mkv|mp4))").expect("valid regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(body) {
        let value = path_decode(cap.get(1).map(|m| m.as_str()).unwrap_or_default().trim());
        if !value.is_empty() && !value.contains('/') && !out.iter().any(|item| item == &value) {
            out.push(value);
        }
    }
    out
}

fn build_episode_label(filename: &str, season: &str) -> String {
    let re = Regex::new(r"\[(?:S\d+-)?E(\d+)]\s*(.+?)\s*\[").expect("valid regex");
    if let Some(cap) = re.captures(filename) {
        let number = cap.get(1).map(|m| m.as_str()).unwrap_or("1");
        let title = cap.get(2).map(|m| m.as_str()).unwrap_or("").trim();
        let suffix = audio_tag(filename)
            .map(|tag| format!(" [{tag}]"))
            .unwrap_or_default();
        format!("Season {season} Ep {number} - {title}{suffix}")
    } else {
        let clean = Regex::new(r"(?i)\[\d{3,4}p].*")
            .expect("valid regex")
            .replace(filename, "")
            .trim()
            .trim_end_matches(".mkv")
            .trim_end_matches(".mp4")
            .to_string();
        if season != "1" && !season.is_empty() {
            format!("Season {season} - {clean}")
        } else {
            clean
        }
    }
}

fn episode_number(filename: &str) -> f32 {
    Regex::new(r"\[(?:S\d+-)?E(\d+)]")
        .expect("valid regex")
        .captures(filename)
        .and_then(|cap| cap.get(1).and_then(|m| m.as_str().parse::<f32>().ok()))
        .unwrap_or(1.0)
}

fn audio_tag(filename: &str) -> Option<String> {
    Regex::new(r"(?i)\[(Dual|Sub|Dub|English Dub)]")
        .expect("valid regex")
        .captures(filename)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

fn quality_label(filename: &str) -> Option<String> {
    Regex::new(r"\[(\d{3,4}p)]")
        .expect("valid regex")
        .captures(filename)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

fn stream_quality(filename: &str, file_size: Option<&str>, request: &Value) -> String {
    let mut label = stream_quality_from_filename(filename);
    if let Some(size) = file_size.filter(|value| !value.is_empty()) {
        label.push_str(" · ");
        label.push_str(size);
    }
    if label == "AV1" {
        label.push_str(" · ");
        label.push_str(&preferred_quality(request).replace(" x ", "x"));
    }
    label
}

fn stream_quality_from_filename(filename: &str) -> String {
    let res = quality_label(filename).unwrap_or_else(|| "AV1".to_string());
    let suffix = audio_tag(filename)
        .map(|tag| format!(" [{tag}]"))
        .unwrap_or_default();
    if res == "AV1" {
        res
    } else {
        format!("AV1 · {res}{suffix}")
    }
}

fn resolution_from_label(label: &str) -> Option<String> {
    Regex::new(r"(\d{3,4}p)")
        .expect("valid regex")
        .captures(label)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default();
        let quality = stream.quality.as_deref().unwrap_or_default();
        let resolution = resolution_from_label(quality)
            .unwrap_or_default()
            .trim_end_matches('p')
            .parse::<i32>()
            .unwrap_or(0);
        (
            i32::from(name.contains(&preferred_link_type(request))),
            i32::from(quality.contains(&preferred_resolution(request))),
            resolution,
        )
    });
    streams.reverse();
}

fn is_preferred(name: &str, request: &Value) -> bool {
    name.contains(&preferred_link_type(request)) || name.contains(&preferred_resolution(request))
}

fn base_url(request: &Value) -> String {
    preference(request, "preferred_domain").unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn preferred_quality(request: &Value) -> String {
    preference(request, "preferred_quality").unwrap_or_else(|| "1920 x 1080".to_string())
}

fn preferred_resolution(request: &Value) -> String {
    preferred_quality(request)
        .split('x')
        .nth(1)
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "1080".to_string())
}

fn preferred_link_type(request: &Value) -> String {
    preference(request, "preferred_link_type").unwrap_or_else(|| "Stream".to_string())
}

fn show_torrent(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("show_torrent"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn preference(request: &Value, key: &str) -> Option<String> {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
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

fn path_encode(value: &str) -> String {
    manatan_shared::sdk::http::url_encode(value).replace('+', "%20")
}

fn path_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug, Default, Deserialize)]
struct DdlResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    stream_link: Option<String>,
    #[serde(default)]
    download_link: Option<String>,
    #[serde(default)]
    torrent_link: Option<String>,
    #[serde(default)]
    watch_link: Option<String>,
    #[serde(default)]
    file_size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct EpisodeItem {
    #[serde(default)]
    num: i32,
    #[serde(default)]
    label: String,
    #[serde(default)]
    href: String,
}

trait TapEmpty {
    fn tap_empty<F: FnOnce() -> Vec<VideoStream>>(self, fallback: F) -> Vec<VideoStream>;
}

impl TapEmpty for Vec<VideoStream> {
    fn tap_empty<F: FnOnce() -> Vec<VideoStream>>(self, fallback: F) -> Vec<VideoStream> {
        if self.is_empty() { fallback() } else { self }
    }
}

const LIST_FIXTURE: &str = r#"
<article class="anime-card">
  <div class="poster-wrap"><img src="/images/sample.jpg"></div>
  <h4><a href="/anime/sample-anime">Sample Anime</a></h4>
</article>
"#;

const SEARCH_FIXTURE: &str = r#"
<ul><li><a href="/anime/sample-anime">Sample Anime</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<main>
  <h1>Sample Anime</h1>
  <img class="anime-poster" src="/images/sample.jpg">
  <div class="description">A local smoke-test fixture.</div>
  <a href="/genre/action">Action</a>
  <button class="season-tab" data-season="1">Season 1</button>
</main>
"#;

const EPISODES_FIXTURE: &str = r#"
<a href="/download/sample-anime/1/1920%20x%201080/%5BE01%5D%20Sample%20Episode%20%5B1080p%5D%20%5BSub%5D.mkv">Episode 1</a>
"#;

export_video_source!(SOURCE);
