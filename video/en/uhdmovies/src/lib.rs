use base64::{Engine as _, engine::general_purpose::STANDARD};
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
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SOURCE: UHDMovies = UHDMovies;
const DEFAULT_BASE_URL: &str = "https://uhdmovies.red";

struct UHDMovies;

impl VideoSource for UHDMovies {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = current_base_url(&request);
        let target = format!("{base}/page/{}/", page(&request));
        let body = get_or_fixture(&target, LIST_FIXTURE, &base);
        Ok(parse_listing(&body, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let base = current_base_url(&request);
        if let Some(path) = path_from_url(query, &base) {
            return Ok(Paged {
                entries: vec![fetch_details(&path, &base)],
                has_next_page: false,
            });
        }
        let clean_query = query.to_lowercase();
        let target = format!(
            "{base}/page/{}/?s={}",
            page(&request),
            url::query_escape(&clean_query)
        );
        let body = get_or_fixture(&target, LIST_FIXTURE, &base);
        Ok(parse_listing(&body, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = current_base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample-movie/".to_string());
        Ok(fetch_details(&path, &base))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let base = current_base_url(&request);
        let path = request_key(&request, "item").unwrap_or_else(|| "/sample-movie/".to_string());
        let body = get_or_fixture(&absolute_url(&path, &base), DETAILS_FIXTURE, &base);
        let episodes = parse_episodes(&body, &path, &base);
        if episodes.is_empty() {
            return Ok(Vec::new());
        }
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let base = current_base_url(&request);
        let episode_key =
            request_raw_key(&request, "episode").unwrap_or_else(|| SAMPLE_EPISODE_KEY.to_string());
        let links = serde_json::from_str::<EpLinks>(&episode_key).unwrap_or_default();
        let mut streams = Vec::new();
        for ep_url in links.urls {
            let quality = ep_url.quality.clone();
            let Some(media_url) = get_media_url(&ep_url, &base) else {
                continue;
            };
            let extracted = extract_video(&media_url, &quality, &request);
            if extracted.is_empty() {
                let gdrive = extract_gdrive_link(&media_url, &quality, &request);
                if gdrive.is_empty() {
                    if let Some(link) = get_direct_link(&media_url, "instant", "/mfile/", &base) {
                        streams.push(stream(
                            &link,
                            &format!("{quality} - GDrive Instant link"),
                            &media_url,
                            &request,
                        ));
                    }
                } else {
                    streams.extend(gdrive);
                }
            } else {
                streams.extend(extracted);
            }
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
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
        let base = current_base_url(&request);
        Ok(request_key(&request, "item").map(|key| absolute_url(&key, &base)))
    }

    fn episode_url(&self, _request: Value) -> ExtensionResult<Option<String>> {
        Ok(None)
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = current_base_url(&request);
        if let Some(path) = path_from_url(input, &base) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&path, &base)),
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

fn get_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(base_from_url(referer), referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn current_base_url(request: &Value) -> String {
    let configured = base_url(request);
    let probe = format!("{}/", configured.trim_end_matches('/'));
    let Ok(response) = client(&configured, &configured)
        .get(&probe)
        .browser_document()
        .send()
    else {
        return configured;
    };
    if response.status == 301 || response.status == 302 {
        if let Some(location) = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("location"))
            .map(|(_, value)| value.trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
        {
            return location;
        }
    }
    configured
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select_all(&doc, "div#content div.gridlove-posts > div.layout-masonry")
            .filter_map(|element| card_item(element, base))
            .collect(),
        has_next_page: select_all(&doc, "div#content > nav.gridlove-pagination > a.next")
            .next()
            .is_some(),
    }
}

fn card_item(element: ElementRef<'_>, base: &str) -> Option<CatalogItem> {
    let href =
        attr(&element, "div.entry-image > a", "href").or_else(|| attr(&element, "a", "href"))?;
    let key = path_key(&href, base);
    let title = attr(&element, "div.entry-image > a", "title")
        .or_else(|| attr(&element, "a", "title"))
        .or_else(|| text(&element, "a"))
        .map(clean_title)
        .unwrap_or_else(|| title_from_path(&key));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: attr(&element, "div.entry-image > a > img", "src")
            .or_else(|| attr(&element, "img", "data-src"))
            .or_else(|| attr(&element, "img", "src"))
            .map(|value| absolute_url(&value, base)),
        url: Some(absolute_url(&key, base)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str, base: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path, base), DETAILS_FIXTURE, base);
    parse_details(&body, path, base).unwrap_or_else(|| CatalogItem {
        key: path_key(path, base),
        title: title_from_path(path),
        url: Some(absolute_url(path, base)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, path: &str, base: &str) -> Option<CatalogItem> {
    let doc = Html::parse_document(body);
    Some(CatalogItem {
        key: path_key(path, base),
        title: select_text(&doc, ".entry-title")
            .map(clean_title)
            .unwrap_or_else(|| title_from_path(path)),
        cover: select_attr(&doc, "meta[property='og:image']", "content")
            .or_else(|| select_attr(&doc, ".entry-image img", "src"))
            .or_else(|| select_attr(&doc, ".featured-thumbnail img", "src"))
            .map(|value| absolute_url(&value, base)),
        description: plot_text(&doc).or_else(|| select_text(&doc, "div.entry-content")),
        status: ItemStatus::Completed,
        url: Some(absolute_url(path, base)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn parse_episodes(body: &str, item_path: &str, base: &str) -> Vec<VideoEpisode> {
    let doc = Html::parse_document(body);
    let rows = select_all(&doc, "p:has(a[href*='?sid=']), p:has(a[href*='r?key='])")
        .filter(|row| {
            row.value()
                .attr("style")
                .is_some_and(|style| style.to_ascii_lowercase().contains("center"))
                && select_all_in(*row, "a[class*='maxbutton']")
                    .next()
                    .is_some()
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Vec::new();
    }
    let first_text = rows.first().map(collect_text).unwrap_or_default();
    let is_series =
        first_text.contains("Episode") || first_text.contains("Zip") || first_text.contains("Pack");

    let quality_regex = Regex::new(r"(?i)\d{3,4}p(?:\s+\w+)?").unwrap();
    let season_regex = Regex::new(r"(?i)[ .]?S(?:eason)?[ .]?(\d{1,2})[ .]?").unwrap();
    let mut triples = Vec::<EpisodeTriple>::new();

    for row in rows {
        let prev_text = previous_element_text(row).unwrap_or_default();
        let row_text = collect_text(&row);
        let quality = quality_regex
            .find(&prev_text)
            .or_else(|| quality_regex.find(&row_text))
            .map(|value| value.as_str().trim().to_string())
            .unwrap_or_else(|| "HD".to_string());
        let default_name = if is_series {
            let season = season_regex
                .captures(&prev_text)
                .and_then(|capture| capture.get(1).map(|value| value.as_str().to_string()))
                .unwrap_or_else(|| "1".to_string());
            let part = Regex::new(r"(?i)Part ?(\d{1,2})")
                .ok()
                .and_then(|regex| regex.captures(&prev_text))
                .and_then(|capture| {
                    capture
                        .get(1)
                        .map(|value| format!(" Pt {}", value.as_str()))
                })
                .unwrap_or_default();
            format!("Season {}{}", season.parse::<u32>().unwrap_or(1), part)
        } else {
            previous_named_heading(row)
                .map(clean_title)
                .unwrap_or_else(|| clean_title(prev_text.lines().next().unwrap_or("Movie")))
        };

        for (index, link) in select_all_in(row, "a[href]").enumerate() {
            let label = collect_text(&link);
            let episode_number = if is_series {
                label
                    .replace("Episode", "")
                    .trim()
                    .parse::<u32>()
                    .unwrap_or((index + 1) as u32)
            } else {
                0
            };
            let Some(url) = link.value().attr("href").filter(|value| !value.is_empty()) else {
                continue;
            };
            let stream_quality = if is_series {
                quality.clone()
            } else {
                format!("{quality} {label}").trim().to_string()
            };
            triples.push(EpisodeTriple {
                group_name: default_name.clone(),
                episode_number,
                url: url.to_string(),
                quality: stream_quality,
            });
        }
    }

    let mut grouped = Vec::<GroupedEpisode>::new();
    for triple in triples {
        if let Some(group) = grouped.iter_mut().find(|group| {
            group.name == triple.group_name && group.episode_number == triple.episode_number
        }) {
            group.urls.push(EpUrl {
                quality: triple.quality,
                url: triple.url,
            });
        } else {
            grouped.push(GroupedEpisode {
                name: triple.group_name,
                episode_number: triple.episode_number,
                urls: vec![EpUrl {
                    quality: triple.quality,
                    url: triple.url,
                }],
            });
        }
    }

    let mut episodes = grouped
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let key = serde_json::to_string(&EpLinks { urls: group.urls }).unwrap_or_default();
            VideoEpisode {
                key,
                title: Some(if is_series {
                    format!("{} Ep {}", group.name, group.episode_number)
                } else {
                    group.name
                }),
                episode_number: Some(if is_series {
                    group.episode_number as f32
                } else {
                    (index + 1) as f32
                }),
                url: Some(absolute_url(item_path, base)),
                language: Some("en".to_string()),
                ..VideoEpisode::default()
            }
        })
        .collect::<Vec<_>>();
    episodes.reverse();
    episodes
}

fn get_media_url(ep_url: &EpUrl, base: &str) -> Option<String> {
    let media_response = if ep_url.url.contains("?sid=") {
        let final_url = redirector_bypass(&ep_url.url, base)?;
        client(base, &ep_url.url).get(final_url).send().ok()?
    } else if ep_url.url.contains("r?key=") {
        client(base, &ep_url.url).get(&ep_url.url).send().ok()?
    } else {
        return None;
    };
    let text = media_response.text.unwrap_or_default();
    let path = text.split("replace(\"").nth(1)?.split('"').next()?;
    if path == "/404" {
        return None;
    }
    Some(format!(
        "https://{}{}",
        host_from_url(&media_response.final_url)?,
        path
    ))
}

fn redirector_bypass(input: &str, base: &str) -> Option<String> {
    let body = client(base, input)
        .get(input)
        .browser_document()
        .referer(base)
        .send_text()
        .ok()?;
    let doc = recursive_landing(body, input, base);
    let html = Html::parse_document(&doc);
    let script = select_all(&html, "script")
        .map(|script| script.inner_html())
        .find(|value| value.contains("/?go=") && value.contains("href"))?;
    let next_url = script.split("\"href\",\"").nth(1)?.split('"').next()?;
    let cookie_name = query_param(next_url, "go")?;
    let cookie_value = script
        .split(&format!("'{cookie_name}', '"))
        .nth(1)?
        .split('\'')
        .next()?;
    let next_body = client(base, input)
        .get(next_url)
        .browser_document()
        .referer(input)
        .header("Cookie", format!("{cookie_name}={cookie_value}"))
        .send_text()
        .ok()?;
    let next_doc = Html::parse_document(&next_body);
    select_attr(&next_doc, "meta[http-equiv]", "content")
        .and_then(|content| content.split("url=").nth(1).map(ToString::to_string))
}

fn recursive_landing(body: String, referer: &str, base: &str) -> String {
    let doc = Html::parse_document(&body);
    let Some(form) = select_all(&doc, "form#landing").next() else {
        return body;
    };
    let Some(action) = form.value().attr("action") else {
        return body;
    };
    let pairs = select_all_in(form, "input")
        .filter_map(|input| {
            Some((
                input.value().attr("name")?.to_string(),
                input.value().attr("value").unwrap_or_default().to_string(),
            ))
        })
        .collect::<Vec<_>>();
    let form = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let Ok(next_body) = client(base, referer)
        .post(absolute_url(action, base))
        .browser_document()
        .referer(referer)
        .form(&form)
        .send_text()
    else {
        return body;
    };
    recursive_landing(next_body, action, base)
}

fn extract_video(media_url: &str, quality: &str, request: &Value) -> Vec<VideoStream> {
    (1..=3)
        .flat_map(|stream_type| extract_worker_links(media_url, quality, stream_type, request))
        .collect()
}

fn extract_worker_links(
    media_url: &str,
    quality: &str,
    stream_type: u8,
    request: &Value,
) -> Vec<VideoStream> {
    let req_link = format!(
        "{}?type={stream_type}",
        media_url.replace("/file/", "/wfile/")
    );
    let body = client(base_from_url(media_url), media_url)
        .get(&req_link)
        .browser_document()
        .referer(media_url)
        .send_text()
        .unwrap_or_default();
    let doc = Html::parse_document(&body);
    let size = select_text(&doc, "div.card-header")
        .and_then(|value| size_label(&value))
        .unwrap_or_default();
    select_all(&doc, "div.card-body div.mb-4 > a")
        .enumerate()
        .filter_map(|(index, link)| {
            let href = link.value().attr("href")?;
            let decoded = if href.contains("workers.dev") {
                href.to_string()
            } else {
                decode_base64(href.split("download?url=").nth(1)?)?
            };
            let label = format!("{quality} - CF {stream_type} Worker {}{size}", index + 1);
            Some(stream(&decoded, &label, media_url, request))
        })
        .collect()
}

fn get_direct_link(url: &str, action: &str, new_path: &str, base: &str) -> Option<String> {
    let body = client(base, url)
        .get(url)
        .browser_document()
        .referer(url)
        .send_text()
        .ok()?;
    let doc = Html::parse_document(&body);
    let script = select_all(&doc, "script")
        .map(|script| script.inner_html())
        .find(|value| value.contains("async function taskaction"))?;
    let key = script.split("key\", \"").nth(1)?.split('"').next()?;
    let target = url.replace("/file/", new_path);
    let host = host_from_url(url)?;
    let response = client(base, url)
        .post(target)
        .header("x-token", host)
        .body(multipart_body(&[
            ("action", action),
            ("key", key),
            ("action_token", ""),
        ]))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .send_text()
        .ok()?;
    serde_json::from_str::<DriveLeechDirect>(&response)
        .ok()
        .and_then(|value| value.url)
}

fn extract_gdrive_link(media_url: &str, quality: &str, request: &Value) -> Vec<VideoStream> {
    let neo_url = get_direct_link(media_url, "direct", "/file/", base_from_url(media_url))
        .unwrap_or_else(|| media_url.to_string());
    let response = client(base_from_url(media_url), media_url)
        .get(&neo_url)
        .browser_document()
        .referer(media_url)
        .send_text()
        .unwrap_or_default();
    let doc = Html::parse_document(&response);
    let Some(button) = select_all(&doc, "div.card-body a.btn").next() else {
        return Vec::new();
    };
    let Some(gd_link) = button.value().attr("href") else {
        return Vec::new();
    };
    let size = size_label(&collect_text(&button)).unwrap_or_default();
    let gd_response = client(base_from_url(gd_link), &neo_url)
        .get(gd_link)
        .browser_document()
        .referer(&neo_url)
        .send_text()
        .unwrap_or_default();
    let gd_doc = Html::parse_document(&gd_response);
    select_attr(&gd_doc, "form#download-form", "action")
        .map(|real_link| {
            vec![stream(
                &real_link,
                &format!("{quality} - Gdrive{size}"),
                gd_link,
                request,
            )]
        })
        .unwrap_or_default()
}

fn stream(url: &str, label: &str, referer: &str, request: &Value) -> VideoStream {
    let quality = first_quality(label).unwrap_or_else(|| label.to_string());
    VideoStream {
        url: url.to_string(),
        name: Some(label.to_string()),
        quality: Some(quality.clone()),
        format: Some("mp4".to_string()),
        resolution: Some(quality.clone()),
        headers: referer_headers(referer),
        stream_kind: Some(VideoStreamKind::Direct),
        preferred: label.contains(preferred_quality(request)),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = preferred_quality(request);
    let asc = preference(request, "preferred_size_sort", "asc") == "asc";
    streams.sort_by(|a, b| {
        let a_pref = a.name.as_deref().unwrap_or_default().contains(preferred);
        let b_pref = b.name.as_deref().unwrap_or_default().contains(preferred);
        b_pref.cmp(&a_pref).then_with(|| {
            let a_size = size_number(a.name.as_deref().unwrap_or_default());
            let b_size = size_number(b.name.as_deref().unwrap_or_default());
            if asc {
                a_size.total_cmp(&b_size)
            } else {
                b_size.total_cmp(&a_size)
            }
        })
    });
}

fn size_label(input: &str) -> Option<String> {
    let start = input.rfind('[')?;
    let end = input[start..].find(']').map(|idx| start + idx)?;
    let value = input[start + 1..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(format!(" - {value}"))
    }
}

fn size_number(input: &str) -> f32 {
    let size = input.rsplit('-').next().unwrap_or_default().trim();
    if let Some(value) = size.strip_suffix("GB").or_else(|| size.strip_suffix("gb")) {
        value
            .trim()
            .parse::<f32>()
            .map(|value| value * 1000.0)
            .unwrap_or(1.0)
    } else {
        size.strip_suffix("MB")
            .or_else(|| size.strip_suffix("mb"))
            .unwrap_or(size)
            .trim()
            .parse::<f32>()
            .unwrap_or(1.0)
    }
}

fn first_quality(input: &str) -> Option<String> {
    Regex::new(r"(?i)\d{3,4}p")
        .ok()?
        .find(input)
        .map(|value| value.as_str().to_string())
}

fn decode_base64(input: &str) -> Option<String> {
    STANDARD
        .decode(input.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn query_param(input: &str, key: &str) -> Option<String> {
    let query = input
        .split('?')
        .nth(1)?
        .split('#')
        .next()
        .unwrap_or_default();
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(hex);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn multipart_body(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!("--{BOUNDARY}\r\n"));
        body.push_str(&format!(
            "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    body.into_bytes()
}

fn base_url(request: &Value) -> String {
    preference(request, "pref_domain_new", DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn preferred_quality(request: &Value) -> &str {
    preference(request, "preferred_quality", "1080p")
}

fn preference<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(default)
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
        .map(|value| path_key(value, &base_url(request)))
}

fn request_raw_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn path_from_url(input: &str, base: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        if trimmed.contains(host_from_url(base)?.as_str()) {
            return Some(path_key(trimmed, base));
        }
        None
    } else if trimmed.starts_with('/') {
        Some(path_key(trimmed, base))
    } else {
        None
    }
}

fn path_key(input: &str, base: &str) -> String {
    if let Some(rest) = input.strip_prefix(base) {
        return path_key(rest, base);
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        let path = input
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
            .unwrap_or_default();
        return path_key(path, base);
    }
    let path = input
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str, base: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(base, input)
    }
}

fn base_from_url(input: &str) -> &str {
    if input.starts_with("http://") || input.starts_with("https://") {
        let scheme_end = input.find("://").map(|index| index + 3).unwrap_or(0);
        let rest = &input[scheme_end..];
        let host_end = rest
            .find('/')
            .map(|index| scheme_end + index)
            .unwrap_or(input.len());
        &input[..host_end]
    } else {
        DEFAULT_BASE_URL
    }
}

fn host_from_url(input: &str) -> Option<String> {
    let rest = input.split("://").nth(1)?;
    Some(rest.split('/').next()?.to_string())
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .next_back()
        .unwrap_or("Movie")
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

fn clean_title(input: impl AsRef<str>) -> String {
    input
        .as_ref()
        .replace("Download", "")
        .split_whitespace()
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

fn previous_element_text(element: ElementRef<'_>) -> Option<String> {
    let mut prev = element.prev_sibling();
    while let Some(node) = prev {
        if let Some(element) = ElementRef::wrap(node) {
            return Some(collect_text(&element));
        }
        prev = node.prev_sibling();
    }
    None
}

fn previous_named_heading(element: ElementRef<'_>) -> Option<String> {
    let mut prev = element.prev_sibling();
    while let Some(node) = prev {
        if let Some(element) = ElementRef::wrap(node) {
            let name = element.value().name();
            let text = collect_text(&element);
            if ["h1", "h2", "h3", "pre"].contains(&name)
                && !text.to_ascii_lowercase().contains("plot")
                && !text.is_empty()
            {
                return Some(text);
            }
        }
        prev = node.prev_sibling();
    }
    None
}

fn plot_text(doc: &Html) -> Option<String> {
    select_all(doc, "pre")
        .map(|element| collect_text(&element))
        .find(|value| value.to_ascii_lowercase().contains("plot"))
}

#[derive(Default, Deserialize, Serialize)]
struct EpLinks {
    urls: Vec<EpUrl>,
}

#[derive(Deserialize, Serialize)]
struct EpUrl {
    quality: String,
    url: String,
}

#[derive(Deserialize)]
struct DriveLeechDirect {
    url: Option<String>,
}

struct EpisodeTriple {
    group_name: String,
    episode_number: u32,
    url: String,
    quality: String,
}

struct GroupedEpisode {
    name: String,
    episode_number: u32,
    urls: Vec<EpUrl>,
}

const BOUNDARY: &str = "----manatanuhdmoviesboundary";
const SAMPLE_EPISODE_KEY: &str = r#"{"urls":[]}"#;

const LIST_FIXTURE: &str = r#"
<div id="content">
  <div class="gridlove-posts">
    <div class="layout-masonry">
      <div class="entry-image">
        <a href="/sample-movie/" title="Download Sample Movie"><img src="/sample.jpg"></a>
      </div>
    </div>
  </div>
  <nav class="gridlove-pagination"><a class="next" href="/page/2/">Next</a></nav>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Download Sample Movie</h1>
<meta property="og:image" content="/sample.jpg">
<div class="entry-content">
  <pre>Plot: Sample description.</pre>
  <h2>Download Sample Movie 1080p</h2>
  <p style="text-align:center"><a class="maxbutton" href="https://links.example/r?key=abc">Download Now</a></p>
</div>
"#;

export_video_source!(SOURCE);
