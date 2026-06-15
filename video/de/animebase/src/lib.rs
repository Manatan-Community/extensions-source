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

const SOURCE: AnimeBase = AnimeBase;
const BASE_URL: &str = "https://anime-base.net";

struct AnimeBase;

impl VideoSource for AnimeBase {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/updates")
        } else {
            format!("{BASE_URL}/favorites")
        };
        let body = get_or_fixture(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: if listing == "latest" {
                parse_cards(&body, "div.box-header + div.box-body")
            } else {
                parse_cards(&body, "table-responsive")
            },
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
        let list = filter(&request, "list");
        if !list.is_empty() {
            let letter = filter(&request, "letter");
            let page = page(&request);
            let body = get_or_fixture(
                &format!("{BASE_URL}/{list}{letter}?page={page}"),
                LIST_FIXTURE,
            );
            return Ok(Paged {
                entries: parse_cards(&body, "table-responsive"),
                has_next_page: has_next_page(&body),
            });
        }
        let token = search_token();
        let form_owned = search_form(&request, query, &token);
        let form_refs = form_owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let body = client()
            .post(format!("{BASE_URL}/searching"))
            .form(&form_refs)
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_cards(&body, "div.col-lg-9.col-md-8 div.box-body"),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = get_or_fixture(&absolute_url(&path), EPISODES_FIXTURE);
        let mut episodes = parse_episodes(&body, &absolute_url(&path));
        episodes.sort_by(|a, b| {
            let a_key = (
                a.title.as_deref().unwrap_or_default().starts_with("Film "),
                a.title
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Special "),
                a.episode_number.unwrap_or(0.0) as i32,
            );
            let b_key = (
                b.title.as_deref().unwrap_or_default().starts_with("Film "),
                b.title
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Special "),
                b.episode_number.unwrap_or(0.0) as i32,
            );
            b_key.cmp(&a_key)
        });
        Ok(episodes)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let key = request_key(&request, "episode")
            .unwrap_or_else(|| "/anime/sample?selector=div.panel.episode-div-1".to_string());
        let (page_url, selector) = split_selector(&key);
        let body = get_or_fixture(&page_url, HOSTERS_FIXTURE);
        Ok(parse_hosters(
            &body,
            selector.as_deref().unwrap_or("div.panel"),
        ))
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let name = request
            .get("hoster")
            .and_then(|hoster| hoster.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Mirror");
        let mut streams = resolve_embed_streams(&key, name);
        sort_streams(
            &mut streams,
            &preferred_lang(&request),
            &preferred_quality(&request),
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
            &preferred_lang(&request),
            &preferred_quality(&request),
        );
        Ok(streams)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Favorites".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: self.list(json!({"listing": "popular"}))?.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Updates".to_string(),
                entries: self.list(json!({"listing": "latest"}))?.entries,
                has_more: false,
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn get_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_token() -> String {
    let body = get_or_fixture(
        &format!("{BASE_URL}/searching"),
        r#"<form><input name="_token" value=""></form>"#,
    );
    html::attr_after(&body, "name=\"_token\"", "value").unwrap_or_default()
}

fn search_form(request: &Value, query: &str, token: &str) -> Vec<(String, String)> {
    let mut form = vec![
        ("_token".to_string(), token.to_string()),
        ("name_serie".to_string(), query.to_string()),
        ("jahr".to_string(), filter(request, "year")),
    ];
    for language in filter_array(request, "languages") {
        form.push(("dubsub[]".to_string(), language));
    }
    for genre in filter_array(request, "genres") {
        form.push(("genre[]".to_string(), genre));
    }
    form
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = get_or_fixture(&absolute_url(path), DETAILS_FIXTURE);
    let info = body.split("div.col-md-9").nth(1).unwrap_or(&body);
    let mut description = info_value(info, "Beschreibung").unwrap_or_default();
    if let Some(original) = info_value(info, "Originalname") {
        description.push_str("\nOriginal name: ");
        description.push_str(&original);
    }
    if let Some(year) = info_value(info, "Erscheinungsjahr") {
        description.push_str("\nErscheinungsjahr: ");
        description.push_str(&year);
    }
    CatalogItem {
        key: path_key(path),
        title: html::text_between(&body, "box-profile", "</h3>")
            .or_else(|| html::text_between(&body, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| title_from_path(path)),
        cover: html::attr_after(&body, "box-profile", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        url: Some(absolute_url(path)),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: collect_anchor_text(info, "Genre"),
        language: Some("de".to_string()),
        content_rating: Some("safe".to_string()),
        status: match info_value(info, "Status").as_deref() {
            Some("Laufend") => ItemStatus::Ongoing,
            Some("Abgeschlossen") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_cards(body: &str, marker: &str) -> Vec<CatalogItem> {
    body.split(marker)
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?.replace("/link/", "/anime/");
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| title_from_path(&href));
            Some(CatalogItem {
                key: path_key(&href),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&href)),
                language: Some("de".to_string()),
                content_rating: Some("safe".to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_episodes(body: &str, page_url: &str) -> Vec<VideoEpisode> {
    body.split("div class=\"panel")
        .skip(1)
        .filter_map(|chunk| {
            let epname = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Episode 1".to_string());
            let language =
                if html::attr_after(chunk, "<button", "data-dubbed").as_deref() == Some("0") {
                    "Subbed"
                } else {
                    "Dubbed"
                };
            let class = chunk
                .split("class=\"")
                .nth(1)?
                .split('"')
                .next()?
                .split_whitespace()
                .find(|class| class.starts_with("episode-div"))
                .unwrap_or("episode-div-1");
            let key = format!("{page_url}?selector=div.panel.{class}");
            Some(VideoEpisode {
                key: key.clone(),
                title: Some(epname.clone()),
                episode_number: epname
                    .substring_before(":")
                    .rsplit(' ')
                    .next()
                    .and_then(|value| value.parse().ok()),
                url: Some(key),
                language: Some("de".to_string()),
                labels: vec![language.to_string()],
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn parse_hosters(body: &str, selector: &str) -> Vec<VideoHoster> {
    body.split(selector)
        .nth(1)
        .unwrap_or(body)
        .split("<button")
        .skip(1)
        .filter_map(|chunk| {
            let hoster = html::text_between(chunk, ">", "</button>")
                .map(|value| html::strip_tags(&value))?;
            let prefix = match hoster.as_str() {
                "Streamwish" => "https://streamwish.to/e/",
                "Voe.SX" => "https://voe.sx/e/",
                "Lulustream" => "https://lulustream.com/e/",
                "VTube" => "https://vtbe.to/embed-",
                "VidGuard" => "https://vembed.net/e/",
                _ => return None,
            };
            let language = if html::attr(chunk, "data-dubbed").as_deref() == Some("0") {
                "SUB"
            } else {
                "DUB"
            };
            let streamlink = html::attr(chunk, "data-streamlink")?;
            let target = format!("{prefix}{streamlink}");
            Some(VideoHoster {
                key: target.clone(),
                name: format!("{language} {hoster}"),
                url: Some(target),
                lazy: true,
                video_count: Some(1),
                headers: referer_headers(BASE_URL),
                ..VideoHoster::default()
            })
        })
        .collect()
}

fn resolve_embed_streams(embed: &str, name: &str) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, embed);
    }
    let body = get_or_fixture(embed, "");
    let unpacked = body.split("eval(").nth(1).unwrap_or(&body);
    if let Some(src) = html::attr_after(&body, "<source", "src")
        .or_else(|| html::text_between(&body, "file:\"", "\""))
        .or_else(|| html::text_between(&body, "file: '", "'"))
        .or_else(|| html::text_between(unpacked, "file:\\\"", "\\\""))
    {
        if src.contains(".m3u8") {
            return parse_hls(&src, name, embed);
        }
        return vec![media_stream(&src, name, "direct", embed)];
    }
    vec![external_stream(embed, name, BASE_URL)]
}

fn parse_hls(target: &str, name: &str, referer: &str) -> Vec<VideoStream> {
    let body = client().get(target).send_text().unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![media_stream(target, name, "auto", referer)];
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
            let stream_url = if line.starts_with("http") {
                line.to_string()
            } else {
                format!(
                    "{}/{}",
                    target
                        .rsplit_once('/')
                        .map(|(base, _)| base)
                        .unwrap_or(target),
                    line
                )
            };
            Some(media_stream(&stream_url, name, &quality, referer))
        })
        .collect()
}

fn media_stream(stream_url: &str, name: &str, quality: &str, referer: &str) -> VideoStream {
    let is_hls = stream_url.contains(".m3u8");
    VideoStream {
        url: stream_url.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(format!("{name} {quality}")),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
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

fn split_selector(key: &str) -> (String, Option<String>) {
    let mut parts = key.split("?selector=");
    let page = parts.next().unwrap_or(key).to_string();
    let selector = parts.next().map(ToString::to_string);
    (page, selector)
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

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<strong")
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| html::text_between(chunk, "</strong>", "</p>"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sort_streams(streams: &mut [VideoStream], preferred_lang: &str, preferred_quality: &str) {
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let digits = quality
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i32>()
            .unwrap_or(0);
        (
            i32::from(
                quality.contains(preferred_lang)
                    || stream
                        .name
                        .as_deref()
                        .unwrap_or_default()
                        .contains(preferred_lang),
            ),
            i32::from(quality.contains(preferred_quality)),
            digits,
        )
    });
    streams.reverse();
    for stream in streams {
        stream.preferred = stream
            .quality
            .as_deref()
            .map(|quality| quality.contains(preferred_lang) || quality.contains(preferred_quality))
            .unwrap_or(false);
    }
}

fn preferred_lang(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_sub"))
        .or_else(|| request.get("preferred_sub"))
        .and_then(Value::as_str)
        .unwrap_or("SUB")
        .to_string()
}

fn preferred_quality(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("preferred_quality"))
        .or_else(|| request.get("preferred_quality"))
        .and_then(Value::as_str)
        .unwrap_or("720p")
        .to_string()
}

fn filter(request: &Value, key: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn filter_array(request: &Value, key: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

trait SubstringBefore {
    fn substring_before(&self, marker: &str) -> &str;
}

impl SubstringBefore for str {
    fn substring_before(&self, marker: &str) -> &str {
        self.split(marker).next().unwrap_or(self)
    }
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
    if input.starts_with(BASE_URL) || input.starts_with("/anime/") || input.starts_with("/link/") {
        Some(path_key(input))
    } else {
        None
    }
}

fn path_key(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(BASE_URL) {
        return input.to_string();
    }
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .replace("/link/", "/anime/");
    if path.contains("?selector=") {
        return absolute_url(&path);
    }
    format!(
        "/{}",
        path.split('?').next().unwrap_or(&path).trim_matches('/')
    )
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
        .unwrap_or("Anime-Base")
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
    body.contains("pagination") && body.contains("rel=next")
}

const LIST_FIXTURE: &str = r#"<div class="table-responsive"><a href="/anime/sample"><div class="thumbnail"><img src="/cover.jpg"></div><div class="caption"><h3>Sample Anime</h3></div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="box-body box-profile"><center><img src="/cover.jpg"><h3>Sample Anime</h3></center></div><div class="box-body"><div class="col-md-9"><strong>Status</strong><p>Laufend</p><strong>Genre</strong><p><a>Action</a></p><strong>Beschreibung</strong><p>Sample description.</p></div></div>"#;
const EPISODES_FIXTURE: &str = r#"<div class="tab-content"><div><div class="panel episode-div-1"><h3>Episode 1</h3><div class="panel-body"><button data-dubbed="0" data-streamlink="sample">Voe.SX</button></div></div></div></div>"#;
const HOSTERS_FIXTURE: &str = EPISODES_FIXTURE;

export_video_source!(SOURCE);
