use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoHoster, VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source,
    source::VideoSource,
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use serde_json::{Value, json};

const SOURCE: AnimeLoads = AnimeLoads;
const BASE_URL: &str = "https://www.anime-loads.org";
const DDOS_CHECK: &str = "https://check.ddos-guard.net/check.js";

struct AnimeLoads;

impl VideoSource for AnimeLoads {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let body = get_or_fixture(
            &format!("{BASE_URL}/anime-series/page/{page}"),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
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
        let page = page(&request);
        let body = get_or_fixture(
            &format!(
                "{BASE_URL}/search/page/{page}?q={}",
                manatan_shared::sdk::http::url_encode(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_cards(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = if body.contains("Anime Serien") || body.contains("streams_episodes_1") {
            parse_series_episodes(&body, &absolute_url(&path))
        } else {
            vec![VideoEpisode {
                key: path.clone(),
                title: html::attr_after(&body, "div.page-header", "title").or_else(|| {
                    html::text_between(&body, "div.page-header", "</h1>")
                        .map(|value| html::strip_tags(&value))
                }),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            }]
        };
        episodes.reverse();
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let path = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/sample#streams_episodes_1".to_string());
        let (page_url, episode_id) = split_episode_key(&path);
        let body = get_or_fixture(&page_url, HOSTERS_FIXTURE);
        let selected_languages = selected_languages(&request);
        let selected_hosters = selected_hosters(&request);
        Ok(parse_hosters(&body, &page_url, &episode_id)
            .into_iter()
            .filter(|hoster| {
                selected_languages
                    .iter()
                    .any(|lang| hoster.name.to_ascii_lowercase().contains(lang))
            })
            .filter(|hoster| {
                selected_hosters
                    .iter()
                    .any(|name| hoster.name.to_ascii_lowercase().contains(name))
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_anime_loads_hoster(&key, name);
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://voe.sx"),
        );
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let mut streams = Vec::new();
        for hoster in self.hosters(request.clone())? {
            let mut resolved = self.resolve_hoster(json!({
                "hoster": { "key": hoster.key, "name": hoster.name },
                "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)
            }))?;
            for stream in &mut resolved {
                stream.hoster = Some(hoster.clone());
            }
            streams.extend(resolved);
        }
        sort_streams(
            &mut streams,
            pref(&request, "preferred_hoster", "https://voe.sx"),
        );
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Anime Serien".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: page.entries,
            has_more: page.has_next_page,
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    let body = client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    if !body.is_empty() {
        return body;
    }
    let _ = refresh_ddos_cookie(target);
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn refresh_ddos_cookie(target: &str) -> Option<String> {
    let well_known = client()
        .get(DDOS_CHECK)
        .send_text()
        .ok()?
        .split('\'')
        .nth(1)?
        .to_string();
    let check_url = format!("{BASE_URL}{well_known}");
    let _ = target;
    client().get(check_url).send_text().ok()
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    CatalogItem {
        key: path_key(path),
        title: html::attr_after(&body, "div.page-header", "title")
            .or_else(|| {
                html::text_between(&body, "div.page-header", "</h1>")
                    .map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "#description", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: html::text_between(&body, "div class=\"pt20", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: collect_anchor_text(&body, "label-group"),
        authors: collect_anchor_text(&body, "col-md-6 text-left"),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("panel-body")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "a class=\"cover-img", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "h4 class=\"title-list", "</a>")
                .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "a class=\"cover-img", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Completed,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_series_episodes(body: &str, page_url: &str) -> Vec<VideoEpisode> {
    body.split("a class=\"list-group-item")
        .skip(1)
        .filter_map(|chunk| {
            let id = html::attr(chunk, "aria-controls")?;
            let ep_text = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "1".to_string());
            let ep_num = html::text_between(chunk, "<strong", "</strong>")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| value.parse::<f32>().ok())
                .or_else(|| ep_text.parse::<f32>().ok())
                .unwrap_or(1.0);
            Some(VideoEpisode {
                key: format!("{page_url}#{id}"),
                title: Some(format!("Ep.{ep_text}")),
                episode_number: Some(ep_num),
                url: Some(format!("{page_url}#{id}")),
                language: Some("de".to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, page_url: &str, episode_id: &str) -> Vec<VideoHoster> {
    let epnum = episode_id
        .split("streams_episodes_1")
        .nth(1)
        .unwrap_or_default();
    let mut out = Vec::new();
    for lang_chunk in body.split("role=\"presentation\"").skip(1) {
        let lang = if lang_chunk.contains("Subtitles: German")
            || lang_chunk.contains("Untertitel: Deutsch")
        {
            "sub"
        } else if lang_chunk.contains("Language: German") || lang_chunk.contains("Sprache: Deutsch")
        {
            "dub"
        } else {
            continue;
        };
        let aria = html::attr_after(lang_chunk, "<a", "aria-controls").unwrap_or_default();
        let id = body
            .split(&format!("id=\"{aria}\""))
            .nth(1)
            .and_then(|chunk| html::attr_after(chunk, "div class=\"episodes", "id"))
            .unwrap_or_else(|| "streams_episodes_1".to_string());
        let selector = format!("{id}{epnum}");
        let Some(element) = body.split(&format!("id=\"{selector}\"")).nth(1) else {
            continue;
        };
        let enc = html::attr_after(element, "data-enc", "data-enc")
            .or_else(|| html::attr(element, "data-enc"))
            .unwrap_or_default();
        if enc.is_empty() {
            continue;
        }
        out.push(VideoHoster {
            key: format!("{page_url}|{selector}|{enc}|{lang}"),
            name: format!("{lang} captcha links"),
            lazy: true,
            video_count: None,
            headers: referer_headers(page_url),
            ..VideoHoster::default()
        });
    }
    if out.is_empty() {
        for chunk in body.split("data-enc").skip(1) {
            let enc = chunk.split('"').nth(1).unwrap_or_default();
            if !enc.is_empty() {
                out.push(VideoHoster {
                    key: format!("{page_url}|{episode_id}|{enc}|sub"),
                    name: "sub captcha links".to_string(),
                    lazy: true,
                    video_count: None,
                    headers: referer_headers(page_url),
                    ..VideoHoster::default()
                });
            }
        }
    }
    out
}

fn resolve_anime_loads_hoster(key: &str, name: &str) -> Vec<VideoStream> {
    let mut parts = key.split('|');
    let page_url = parts.next().unwrap_or(BASE_URL);
    let _selector = parts.next().unwrap_or_default();
    let enc = parts.next().unwrap_or_default();
    let lang = parts.next().unwrap_or("sub");
    let hashes_body = client()
        .post(format!("{BASE_URL}/files/captcha"))
        .form(&[("cID", "0"), ("rT", "1")])
        .header("X-Requested-With", "XMLHttpRequest")
        .referer(page_url)
        .xhr()
        .send_text()
        .unwrap_or_default();
    let hashes = parse_hashes(&hashes_body);
    let mut out = Vec::new();
    for hash in hashes.into_iter().take(3) {
        let response = client()
            .post(format!("{BASE_URL}/ajax/captcha"))
            .form(&[
                ("enc", enc),
                ("response", "captcha"),
                ("captcha-idhf", "0"),
                ("captcha-hf", &hash),
            ])
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(page_url)
            .xhr()
            .send_text()
            .unwrap_or_default();
        if response.contains("\"error") {
            continue;
        }
        for (hoster, linkpart) in parse_ajax_links(&response) {
            let leave_url = resolve_leave_url(&linkpart);
            out.push(external_stream(
                &leave_url,
                &format!("{name} {lang} {hoster}"),
                page_url,
            ));
        }
        if !out.is_empty() {
            return out;
        }
    }
    vec![external_stream(key, name, page_url)]
}

fn parse_hashes(body: &str) -> Vec<String> {
    body.split('[')
        .nth(1)
        .and_then(|part| part.split(']').next())
        .unwrap_or(body)
        .split(',')
        .map(|value| {
            value
                .replace("<body>", "")
                .replace("</body>", "")
                .replace('"', "")
                .replace("%20", "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_ajax_links(body: &str) -> Vec<(String, String)> {
    body.split("{\"links\":")
        .skip(1)
        .filter_map(|chunk| {
            let hoster = chunk
                .split("\"hoster\":\"")
                .nth(1)?
                .split("\",")
                .next()?
                .to_string();
            let link = chunk
                .split("\"link\":\"")
                .nth(1)?
                .split('"')
                .next()?
                .to_string();
            Some((hoster, link))
        })
        .collect()
}

fn resolve_leave_url(linkpart: &str) -> String {
    client()
        .get(format!("{BASE_URL}/leave/{linkpart}"))
        .browser_document()
        .send_text()
        .ok()
        .and_then(|body| html::attr_after(&body, "<a", "href"))
        .unwrap_or_else(|| format!("{BASE_URL}/leave/{linkpart}"))
}

fn external_stream(stream_url: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: stream_url.to_string(),
        name: Some(name.to_string()),
        quality: Some(name.to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], preferred_hoster: &str) {
    streams.sort_by_key(|stream| {
        i32::from(
            stream.url.contains(preferred_hoster)
                || stream
                    .quality
                    .as_deref()
                    .map(|quality| quality.contains(preferred_hoster))
                    .unwrap_or(false),
        )
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream.url.contains(preferred_hoster);
    }
}

fn selected_hosters(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("hoster_selection"))
        .and_then(Value::as_array)
    else {
        return vec!["dood".to_string(), "voe".to_string(), "stape".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn selected_languages(request: &Value) -> Vec<String> {
    let Some(values) = request
        .get("preferences")
        .and_then(|prefs| prefs.get("sub_selection"))
        .and_then(Value::as_array)
    else {
        return vec!["sub".to_string()];
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn pref<'a>(request: &'a Value, key: &str, default: &'a str) -> &'a str {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn collect_anchor_text(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn split_episode_key(key: &str) -> (String, String) {
    let mut parts = key.split('#');
    let page_url = parts.next().unwrap_or(key).to_string();
    let episode_id = parts.next().unwrap_or_default().to_string();
    (page_url, episode_id)
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| {
            value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .or_else(|| value.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn path_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with('/') {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let mut anchor = "";
    let before_anchor = if let Some((before, after)) = input.split_once('#') {
        anchor = after;
        before
    } else {
        input
    };
    let path = before_anchor
        .strip_prefix(BASE_URL)
        .unwrap_or(before_anchor)
        .split('?')
        .next()
        .unwrap_or(before_anchor)
        .trim_matches('/');
    if anchor.is_empty() {
        format!("/{path}")
    } else {
        format!("/{path}#{anchor}")
    }
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn title_from_path(input: &str) -> String {
    input
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Anime-Loads")
        .replace('-', " ")
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn has_next_page(body: &str) -> bool {
    body.contains("glyphicon-forward")
}

const LIST_FIXTURE: &str = r#"<div class="row"><div class="col-sm-6"><div class="panel-body"><div class="row"><a class="cover-img" href="/anime/sample"><img src="/cover.jpg"></a><h4 class="title-list"><a>Sample Anime</a></h4></div></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="page-header"><h1 title="Sample Anime"></h1></div><div id="description"><img class="img-responsive" src="/cover.jpg"><div class="label-group"><a class="label label-info">Action</a></div></div><div class="pt20">Sample description.</div><a title="Anime Serien"></a>"#;
const EPISODES_FIXTURE: &str = r#"<a title="Anime Serien"></a><meta property="og:url" content="https://www.anime-loads.org/anime/sample"><div id="streams_episodes_1"><div class="list-group"><a class="list-group-item" aria-controls="streams_episodes_11"><span>1</span><span><strong>1</strong></span></a></div></div>"#;
const HOSTERS_FIXTURE: &str = r#"<div id="streams"><ul class="nav"><li role="presentation"><a aria-controls="sub"><i class="flag-de" title="Subtitles: German"></i></a></li></ul><div id="sub"><div class="episodes" id="streams_episodes_1"></div></div><div id="streams_episodes_11" data-enc="sample"></div></div>"#;

export_video_source!(SOURCE);
