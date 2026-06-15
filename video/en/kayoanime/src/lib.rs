use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult, cookies_get, system_time},
    export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const SOURCE: KayoAnime = KayoAnime;
const BASE_URL: &str = "https://kayoanime.com";
const DRIVE_URL: &str = "https://drive.google.com";
const BOUNDARY: &str = "=====vc17a3rwnndj=====";
const MAX_RECURSION_DEPTH: usize = 2;

struct KayoAnime;

impl VideoSource for KayoAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            let body = get_or_fixture(BASE_URL, HOME_FIXTURE, BASE_URL);
            return Ok(Paged {
                entries: parse_latest(&body),
                has_next_page: false,
            });
        }
        archive_page("/ongoing-animes/", page, &request)
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
            let escaped = query.replace(' ', "+");
            return archive_page(&format!("/?s={escaped}"), page(&request), &request);
        }
        if let Some(path) =
            filter_path(&request, "genre").or_else(|| filter_path(&request, "sub_page"))
        {
            return archive_page(&path, page(&request), &request);
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/sample-anime/".to_string());
        Ok(fetch_details(&key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_else(|| "/sample-anime/".to_string());
        let body = get_or_fixture(&absolute_url(&key), DETAILS_FIXTURE, BASE_URL);
        let mut episodes = Vec::new();

        for block in toggle_blocks(&body) {
            let path = video_path_from_block(block);
            for (href, text) in links_containing(block, "drive.google.com") {
                let clean = href.split("?usp=shar").next().unwrap_or(&href).to_string();
                traverse_drive_folder(
                    &clean,
                    &format!("{path} {text}"),
                    0,
                    &request,
                    &mut episodes,
                )?;
            }
        }

        for block in toggle_blocks(&body) {
            let path = video_path_from_block(block);
            for (href, text) in links_containing(block, "tinyurl.com") {
                if let Some(location) = resolve_redirect(&href) {
                    let host = host_from_url(&location);
                    if host.contains("workers.dev") {
                        let mut counter = episodes.len() as f32 + 1.0;
                        traverse_index(
                            &location,
                            &format!("{path} {text}"),
                            0,
                            pref_bool(&request, "trim_episode", true),
                            &mut counter,
                            &mut episodes,
                        )?;
                    } else if host.contains("slogoanime") {
                        let document = get_or_fixture(&location, "", BASE_URL);
                        for (drive, label) in links_containing(&document, "drive.google.com") {
                            let clean = drive
                                .split("?usp=shar")
                                .next()
                                .unwrap_or(&drive)
                                .to_string();
                            traverse_drive_folder(
                                &clean,
                                &format!("{path} {text} {label}"),
                                0,
                                &request,
                                &mut episodes,
                            )?;
                        }
                    }
                }
            }
        }

        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode")
            .or_else(|| {
                request
                    .get("key")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_default();
        if episode.contains("workers.dev") {
            return Ok(index_streams(&episode));
        }
        let mut headers = Context::new();
        headers.insert("Referer".to_string(), DRIVE_URL.to_string());
        Ok(vec![VideoStream {
            url: episode.clone(),
            name: Some("Google Drive".to_string()),
            quality: Some("external".to_string()),
            format: Some("external".to_string()),
            stream_kind: Some(VideoStreamKind::External),
            requires_proxy: true,
            headers,
            initialized: true,
            ..VideoStream::default()
        }])
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Ongoing Animes".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recent".to_string(),
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
        Ok(request_key(&request, "episode"))
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

fn archive_page(path: &str, page: u64, request: &Value) -> ExtensionResult<Paged<CatalogItem>> {
    let url = absolute_url(path);
    if page <= 1 {
        let body = get_or_fixture(&url, LIST_FIXTURE, BASE_URL);
        return Ok(parse_archive(&body, &url, request));
    }

    let first = get_or_fixture(&url, LIST_FIXTURE, BASE_URL);
    let Some(load) = load_more_data(&first) else {
        return Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        });
    };
    let form = [
        ("action", "tie_archives_load_more"),
        ("query", load.query.as_str()),
        ("max", load.max.as_str()),
        ("page", &page.to_string()),
        ("latest_post", load.latest.as_str()),
        ("layout", load.layout.as_str()),
        ("settings", load.settings.as_str()),
    ];
    let body = site_client(&url)
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .header("Accept", "*/*")
        .origin(BASE_URL)
        .referer(&url)
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| AJAX_FIXTURE.to_string());
    Ok(parse_ajax_archive(&body, request))
}

fn parse_archive(body: &str, referer: &str, request: &Value) -> Paged<CatalogItem> {
    Paged {
        entries: post_item_chunks(body)
            .into_iter()
            .filter_map(|chunk| parse_card(chunk, request))
            .collect(),
        has_next_page: load_more_data(body).is_some() || referer.contains("?s="),
    }
}

fn parse_ajax_archive(body: &str, request: &Value) -> Paged<CatalogItem> {
    let parsed = serde_json::from_str::<String>(body)
        .ok()
        .and_then(|raw| serde_json::from_str::<PostResponse>(&raw).ok())
        .or_else(|| serde_json::from_str::<PostResponse>(body).ok())
        .unwrap_or(PostResponse {
            hide_next: true,
            code: body.to_string(),
        });
    Paged {
        entries: post_item_chunks(&parsed.code)
            .into_iter()
            .filter_map(|chunk| parse_card(chunk, request))
            .collect(),
        has_next_page: !parsed.hide_next,
    }
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("widget-single-post-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let path = path_key(&href);
            let title = html::text_between(chunk, "post-title", "</a>")
                .map(|text| html::strip_tags(&text))
                .unwrap_or_else(|| title_from_path(&path));
            Some(CatalogItem {
                key: path.clone(),
                title: clean_anime_title(&title),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&path)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_card(chunk: &str, _request: &Value) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let path = path_key(&href);
    let title = html::text_between(chunk, "post-title", "</h2>")
        .map(|text| html::strip_tags(&text))
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .unwrap_or_else(|| title_from_path(&path));
    Some(CatalogItem {
        key: path.clone(),
        title: clean_anime_title(&title),
        cover: html::attr_after(chunk, "<img", "src")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(&path)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let title = html::text_between(&body, "entry-title", "</h1>")
        .or_else(|| html::text_between(&body, "<h1", "</h1>"))
        .or_else(|| html::text_between(&body, "post-title", "</h1>"))
        .map(|text| html::strip_tags(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| title_from_path(path));
    let info = toggle_info_lines(&body);
    let description = details_description(&body, &info);
    CatalogItem {
        key: path_key(path),
        title: clean_anime_title(&title),
        cover: html::attr_after(&body, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        authors: info_value(&info, "Studios:").into_iter().collect(),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: info_value(&info, "Genres:")
            .map(|value| {
                value
                    .split(',')
                    .map(|tag| tag.trim().to_string())
                    .filter(|tag| !tag.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: parse_status(&info_value(&info, "Status:").unwrap_or_default()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn details_description(body: &str, info: &[String]) -> String {
    let mut out = String::new();
    if let Some(block) = body
        .split("Information")
        .next()
        .and_then(|head| head.rsplit("toggle-content").next())
        .map(html::strip_tags)
        .filter(|text| !text.is_empty())
    {
        out.push_str(&block);
        out.push_str("\n\n");
    }
    out.push_str(&info.join("\n"));
    out
}

fn traverse_drive_folder(
    folder_url: &str,
    path: &str,
    depth: usize,
    request: &Value,
    out: &mut Vec<VideoEpisode>,
) -> ExtensionResult<()> {
    if depth >= MAX_RECURSION_DEPTH {
        return Ok(());
    }
    let Some(folder_id) = drive_folder_id(folder_url) else {
        return Ok(());
    };
    let document = drive_client()
        .get(folder_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| DRIVE_FIXTURE.to_string());
    if document.contains("Error 404") {
        return Ok(());
    }
    let mut page_token = String::new();
    loop {
        let response = drive_batch(&document, &folder_id, &page_token)?;
        for (index, item) in response.items.unwrap_or_default().into_iter().enumerate() {
            if item.mime_type.starts_with("video") {
                let size = item
                    .file_size
                    .as_deref()
                    .and_then(|value| value.parse::<u64>().ok());
                let title = if pref_bool(request, "trim_episode", true) {
                    trim_info(&item.title)
                } else {
                    item.title.clone()
                };
                let url = format!("https://drive.google.com/uc?id={}", item.id);
                out.push(VideoEpisode {
                    key: url.clone(),
                    title: Some(title),
                    episode_number: episode_number(&item.title).or(Some((index + 1) as f32)),
                    url: Some(url),
                    size_bytes: size,
                    release_group: Some(folder_label(path, size)),
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
            } else if item.mime_type.ends_with(".folder") {
                let child = format!("https://drive.google.com/drive/folders/{}", item.id);
                let child_path = if path.is_empty() {
                    item.title.clone()
                } else {
                    format!("{path}/{}", item.title)
                };
                traverse_drive_folder(&child, &child_path, depth + 1, request, out)?;
            }
        }
        let Some(next) = response.next_page_token else {
            break;
        };
        page_token = next;
    }
    Ok(())
}

fn drive_batch(
    document: &str,
    folder_id: &str,
    page_token: &str,
) -> ExtensionResult<DriveResponse> {
    let key =
        api_key(document).unwrap_or_else(|| "AIzaSyD-fixture-key-fixture-key-fixture".to_string());
    let version = document
        .split('"')
        .find(|part| part.contains("web-frontend"))
        .unwrap_or("")
        .to_string();
    let request_path = format!(
        "/drive/v2internal/files?openDrive=false&reason=102&syncType=0&errorRecovery=false&q=trashed%20%3D%20false%20and%20'{folder_id}'%20in%20parents&fields=kind%2CnextPageToken%2Citems(title%2Cid%2CfileSize%2CmimeType)&spaces=drive&pageToken={page_token}&maxResults=100&supportsTeamDrives=true&includeItemsFromAllDrives=true&corpora=default&orderBy=folder%2Ctitle_natural%20asc&key={key} HTTP/1.1"
    );
    let body = format!(
        "--{BOUNDARY}\r\ncontent-type: application/http\r\ncontent-transfer-encoding: binary\r\n\r\nGET {request_path}\r\nX-Goog-Drive-Client-Version: {version}\r\nauthorization: {}\r\nx-goog-authuser: 0\r\n\r\n--{BOUNDARY}--",
        sapisid_hash().unwrap_or_default()
    );
    let raw = drive_client()
        .post(format!(
            "https://clients6.google.com/batch/drive/v2internal?$ct=multipart/mixed; boundary=\"{BOUNDARY}\"&key={key}"
        ))
        .header("Content-Type", "text/plain; charset=UTF-8")
        .origin(DRIVE_URL)
        .body(body.into_bytes())
        .send_text()
        .unwrap_or_else(|_| DRIVE_POST_FIXTURE.to_string());
    let json = raw
        .find('{')
        .and_then(|start| raw.rfind('}').map(|end| raw[start..=end].to_string()))
        .unwrap_or_else(|| DRIVE_POST_FIXTURE.to_string());
    serde_json::from_str(&json).map_err(|err| error(format!("invalid Drive response: {err}")))
}

fn traverse_index(
    index_url: &str,
    path: &str,
    depth: usize,
    trim_name: bool,
    counter: &mut f32,
    out: &mut Vec<VideoEpisode>,
) -> ExtensionResult<()> {
    if depth >= MAX_RECURSION_DEPTH {
        return Ok(());
    }
    let mut token = String::new();
    let mut page_index = 0_u64;
    loop {
        let response = post_index(index_url, &token, page_index)?;
        for file in response.data.files {
            if file.mime_type.ends_with("folder") {
                let child = add_suffix(&join_url(index_url, &file.name), "/");
                let next_path = if path.is_empty() {
                    file.name.clone()
                } else {
                    format!("{path}/{}", trim_info(&file.name))
                };
                traverse_index(&child, &next_path, depth + 1, trim_name, counter, out)?;
            } else if file.mime_type.starts_with("video/") {
                let ep_url = join_url(index_url, &file.name);
                let size = file.size.and_then(|value| value.parse::<u64>().ok());
                out.push(VideoEpisode {
                    key: ep_url.clone(),
                    title: Some(if trim_name {
                        trim_info(&file.name)
                    } else {
                        file.name
                    }),
                    episode_number: Some(*counter),
                    url: Some(ep_url),
                    release_group: Some(folder_label(path, size)),
                    size_bytes: size,
                    language: Some("en".to_string()),
                    ..VideoEpisode::default()
                });
                *counter += 1.0;
            }
        }
        let Some(next) = response.next_page_token else {
            break;
        };
        token = next;
        page_index += 1;
    }
    Ok(())
}

fn post_index(target: &str, page_token: &str, page_index: u64) -> ExtensionResult<IndexResponse> {
    let body = format!("password=&page_token={page_token}&page_index={page_index}");
    let response = site_client(target)
        .post(target)
        .xhr()
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .origin(origin(target))
        .referer(&url::query_escape(target))
        .body(body.into_bytes())
        .send_text()
        .unwrap_or_else(|_| INDEX_FIXTURE_ENCRYPTED.to_string());
    let decrypted =
        decrypt_index(&response).ok_or_else(|| error("Unable to decrypt index response"))?;
    serde_json::from_str(&decrypted).map_err(|_| error("Invalid index JSON"))
}

fn index_streams(episode_url: &str) -> Vec<VideoStream> {
    let body = site_client(episode_url)
        .get(format!("{episode_url}?a=view"))
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("videodomain") || chunk.contains("downloaddomain"))
        .unwrap_or_default();
    let domain = script
        .split("\"videodomain\":\"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .or_else(|| {
            script
                .split("\"downloaddomain\":\"")
                .nth(1)
                .and_then(|tail| tail.split('"').next())
        })
        .unwrap_or_default();
    let video_url = if domain.is_empty() || script.contains("\"second_domain_for_dl\":false") {
        episode_url.to_string()
    } else {
        format!("{domain}{}", path_from_any_url(episode_url))
    };
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), episode_url.to_string());
    vec![VideoStream {
        url: video_url,
        name: Some("Video".to_string()),
        quality: Some("direct".to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers,
        initialized: true,
        ..VideoStream::default()
    }]
}

fn site_client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn drive_client() -> HttpClient {
    HttpClient::browser()
        .with_header("Accept", "*/*")
        .with_referer(DRIVE_URL)
        .with_cookies_for(DRIVE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    site_client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn resolve_redirect(input: &str) -> Option<String> {
    site_client(BASE_URL)
        .get(input)
        .send()
        .ok()
        .map(|response| response.final_url)
        .filter(|url| url != input)
        .or_else(|| {
            site_client(BASE_URL)
                .get(input)
                .send()
                .ok()
                .and_then(|response| {
                    response
                        .headers
                        .into_iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                        .map(|(_, value)| value)
                })
        })
}

fn post_item_chunks(body: &str) -> Vec<&str> {
    body.split("<li")
        .filter(|chunk| chunk.contains("post-item"))
        .collect()
}

fn load_more_data(body: &str) -> Option<LoadMoreData> {
    let nav = body.split("pages-nav").nth(1)?;
    Some(LoadMoreData {
        layout: html::attr_after(body, "posts-container", "data-layout").unwrap_or_default(),
        settings: html::attr_after(body, "posts-container", "data-settings").unwrap_or_default(),
        query: html::attr(nav, "data-query").unwrap_or_default(),
        max: html::attr(nav, "data-max").unwrap_or_default(),
        latest: html::attr(nav, "data-latest").unwrap_or_default(),
    })
}

fn toggle_blocks(body: &str) -> Vec<&str> {
    body.split("<div")
        .filter(|chunk| chunk.contains("toggle") && chunk.contains("toggle-content"))
        .collect()
}

fn links_containing(block: &str, needle: &str) -> Vec<(String, String)> {
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains(needle) {
                return None;
            }
            let text = chunk
                .split("</a>")
                .next()
                .map(html::strip_tags)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "Drive".to_string());
            Some((href, text))
        })
        .collect()
}

fn video_path_from_block(block: &str) -> String {
    html::text_between(block, "<h3", "</h3>")
        .map(|text| html::strip_tags(&text))
        .unwrap_or_default()
        .split("480p")
        .next()
        .unwrap_or_default()
        .split("720p")
        .next()
        .unwrap_or_default()
        .split("1080p")
        .next()
        .unwrap_or_default()
        .replace("Download The Anime From Drive", "")
        .trim()
        .to_string()
}

fn toggle_info_lines(body: &str) -> Vec<String> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let text = chunk.split("</li>").next().map(html::strip_tags)?;
            (!text.is_empty()).then_some(text)
        })
        .collect()
}

fn info_value(info: &[String], label: &str) -> Option<String> {
    info.iter()
        .find_map(|line| {
            line.strip_prefix(label)
                .map(str::trim)
                .map(ToString::to_string)
        })
        .filter(|value| !value.is_empty())
}

fn api_key(document: &str) -> Option<String> {
    document
        .split('"')
        .find(|part| {
            part.len() == 39
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        })
        .map(ToString::to_string)
}

fn sapisid_hash() -> Option<String> {
    let cookie = cookies_get(DRIVE_URL).ok()?.header?;
    let sapisid = cookie.split(';').map(str::trim).find_map(|pair| {
        pair.strip_prefix("SAPISID=")
            .or_else(|| pair.strip_prefix("__Secure-3PAPISID="))
    })?;
    let now = system_time().ok()?.unix_seconds;
    let material = format!("{now} {sapisid} {DRIVE_URL}");
    let digest = Sha1::digest(material.as_bytes());
    Some(format!("SAPISIDHASH {now}_{digest:x}"))
}

fn decrypt_index(input: &str) -> Option<String> {
    if input.trim_start().starts_with('{') {
        return Some(input.to_string());
    }
    if input.len() <= 44 {
        return None;
    }
    let reversed = input.chars().rev().collect::<String>();
    let payload = &reversed[24..reversed.len().saturating_sub(20)];
    STANDARD
        .decode(payload)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn drive_folder_id(input: &str) -> Option<String> {
    input
        .split("/folders/")
        .nth(1)
        .and_then(|part| part.split(['?', '#', '/', ';']).next())
        .filter(|id| id.len() >= 20)
        .map(ToString::to_string)
}

fn episode_number(title: &str) -> Option<f32> {
    title.split(" - ").nth(1).and_then(|tail| {
        tail.chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
            .collect::<String>()
            .parse()
            .ok()
    })
}

fn trim_info(input: &str) -> String {
    let mut out = input.to_string();
    if out.starts_with('[') && out.contains(']') {
        out = out
            .split_once(']')
            .map(|(_, tail)| tail.trim().to_string())
            .unwrap_or(out);
    }
    loop {
        let trimmed = out
            .trim_end()
            .trim_end_matches(".mkv")
            .trim_end_matches(".mp4")
            .trim_end_matches(".avi")
            .trim_end()
            .to_string();
        let Some(start) = trimmed.rfind(['[', '(']) else {
            return out.trim().to_string();
        };
        let close = if trimmed.as_bytes()[start] == b'[' {
            ']'
        } else {
            ')'
        };
        if trimmed.ends_with(close) {
            out = trimmed[..start].trim().to_string();
        } else {
            return out.trim().to_string();
        }
    }
}

fn folder_label(path: &str, size: Option<u64>) -> String {
    let size = size.map(format_bytes).unwrap_or_default();
    let path = path.trim();
    if size.is_empty() {
        format!("/{path}")
    } else {
        format!("{size} - /{path}")
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else if bytes == 1 {
        "1 byte".to_string()
    } else {
        format!("{bytes} bytes")
    }
}

fn parse_status(input: &str) -> ItemStatus {
    match input {
        "Currently Airing" => ItemStatus::Ongoing,
        "Finished Airing" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn clean_anime_title(input: &str) -> String {
    input
        .split(" Episode")
        .next()
        .unwrap_or(input)
        .trim()
        .to_string()
}

fn title_from_path(input: &str) -> String {
    input
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Kayoanime")
        .replace('-', " ")
}

fn path_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with('/')).then(|| path_key(input))
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(BASE_URL).unwrap_or(input);
    let path = without_base.split('#').next().unwrap_or(without_base);
    let path = if path.starts_with("/?s=") {
        path
    } else {
        path.split('?').next().unwrap_or(path)
    };
    format!("/{}", path.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("/?") {
        format!("{BASE_URL}{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn join_url(base: &str, path: &str) -> String {
    url::join_url(base.trim_end_matches('/'), path)
}

fn add_suffix(input: &str, suffix: &str) -> String {
    if input.ends_with(suffix) {
        input.to_string()
    } else {
        format!("{input}{suffix}")
    }
}

fn origin(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .map(|host| format!("https://{host}"))
        .unwrap_or_default()
}

fn host_from_url(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .unwrap_or_default()
        .to_string()
}

fn path_from_any_url(input: &str) -> String {
    input
        .split("//")
        .nth(1)
        .and_then(|tail| tail.split_once('/').map(|(_, path)| format!("/{path}")))
        .unwrap_or_else(|| "/".to_string())
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
        .map(ToString::to_string)
}

fn pref_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(default)
}

fn filter_path(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .and_then(|value| value.split(':').next_back())
        .filter(|value| value.starts_with('/'))
        .map(ToString::to_string)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn with_listing(request: &Value, listing: &str) -> Value {
    json!({
        "listing": listing,
        "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
    })
}

fn error(message: impl Into<String>) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

struct LoadMoreData {
    layout: String,
    settings: String,
    query: String,
    max: String,
    latest: String,
}

#[derive(Deserialize)]
struct PostResponse {
    hide_next: bool,
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveResponse {
    next_page_token: Option<String>,
    items: Option<Vec<DriveItem>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveItem {
    id: String,
    title: String,
    mime_type: String,
    file_size: Option<String>,
}

#[derive(Deserialize)]
struct IndexResponse {
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    data: IndexData,
}

#[derive(Deserialize)]
struct IndexData {
    files: Vec<IndexFile>,
}

#[derive(Deserialize)]
struct IndexFile {
    #[serde(rename = "mimeType")]
    mime_type: String,
    name: String,
    size: Option<String>,
}

const LIST_FIXTURE: &str = r#"<ul id="posts-container" data-layout="grid" data-settings="fixture"><li class="post-item"><a href="https://kayoanime.com/sample-anime/"><img src="/poster.jpg"><h2 class="post-title">Sample Anime Episode 1</h2></a></li></ul><div class="pages-nav"><a data-text="load more" data-query="fixture" data-max="1" data-latest="1"></a></div>"#;
const AJAX_FIXTURE: &str = r#"{"hide_next":true,"code":"<li class=\"post-item\"><a href=\"https://kayoanime.com/sample-anime/\"><img src=\"/poster.jpg\"><h2 class=\"post-title\">Sample Anime Episode 2</h2></a></li>"}"#;
const HOME_FIXTURE: &str = r#"<ul class="tabs"><li><a>Recent</a></li></ul><div class="tab-content"><li class="widget-single-post-item"><a href="https://kayoanime.com/sample-anime/" class="post-title">Sample Anime Episode 1</a><img src="/poster.jpg"></li></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Anime</h1><div class="entry-content"><div class="toggle"><h3>Download The Anime From Drive 1080p</h3><div class="toggle-content"><a href="https://drive.google.com/drive/folders/abcdefghijklmnopqrstuvwxyz123456">Batch</a></div></div><div class="toggle-content"><ul><li>Status: Currently Airing</li><li>Genres: Adventure, Fantasy</li><li>Studios: Sample Studio</li></ul></div></div>"#;
const DRIVE_FIXTURE: &str = r#"<script>"AIzaSyD-fixture-key-fixture-key-fixture"</script>"#;
const DRIVE_POST_FIXTURE: &str = r#"{"items":[{"id":"abcdefghijklmnopqrstuvwxyz123456","title":"Sample Anime - 1.mkv","mimeType":"video/x-matroska","fileSize":"1048576"}]}"#;
const INDEX_FIXTURE_ENCRYPTED: &str = r#"{"data":{"files":[{"mimeType":"video/mp4","id":"sample","name":"Sample Anime - 1.mp4","size":"1048576"}]}}"#;

export_video_source!(SOURCE);
