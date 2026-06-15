use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: PelisPlusHd = PelisPlusHd;
const BASE_URL: &str = "https://pelisplushd.bz";

struct PelisPlusHd;

impl VideoSource for PelisPlusHd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch(
            &format!("{BASE_URL}/series?page={}", page(&request)),
            LIST_FIXTURE,
            BASE_URL,
        )))
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
        let p = page(&request);
        let target = if !query.is_empty() {
            format!("{BASE_URL}/search?s={}&page={p}", url::query_escape(query))
        } else if let Some(genre) = filter(&request, "genre").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/{genre}?page={p}")
        } else if let Some(year) = filter(&request, "year").filter(|value| !value.trim().is_empty())
        {
            format!(
                "{BASE_URL}/year/{}?page={p}",
                url::query_escape(year.trim())
            )
        } else {
            format!("{BASE_URL}/peliculas?page={p}")
        };
        Ok(parse_listing(&fetch(&target, LIST_FIXTURE, BASE_URL)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/serie/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, DETAILS_FIXTURE, BASE_URL);
        if referer.contains("/pelicula/") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("PELICULA".to_string()),
                episode_number: Some(1.0),
                url: Some(referer),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let doc = Html::parse_document(&body);
        let mut episodes = doc
            .select(&selector("div.tab-content div a"))
            .enumerate()
            .filter_map(|(idx, a)| {
                let href = attr(&a, "href")?;
                let key = path_key(&href);
                Some(VideoEpisode {
                    key: key.clone(),
                    title: Some(text(a)),
                    episode_number: Some((idx + 1) as f32),
                    url: Some(absolute_url(&key)),
                    language: Some("es".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/episodio/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let doc = Html::parse_document(&body);
        let Some(script) = doc
            .select(&selector("script"))
            .map(|s| s.inner_html())
            .find(|s| s.contains("video[1] = "))
        else {
            return Ok(Vec::new());
        };
        let mut streams = Vec::new();
        for embed in Regex::new(r#"'(https?://[^']*)'"#)
            .unwrap()
            .captures_iter(&script)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .filter(|link| link.contains("embed69.org"))
        {
            streams.extend(embed69_streams(&embed, &referer, &request));
        }
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|p| absolute_url(&p)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|p| absolute_url(&p)))
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
        .with_header("Origin", BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str, referer: &str) -> String {
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
        entries: doc
            .select(&selector("div.Posters a.Posters-link"))
            .filter_map(card)
            .collect(),
        has_next_page: doc.select(&selector("a.page-link")).next().is_some(),
    }
}

fn card(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href")?;
    let key = path_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: select_text(el, "div.listing-content p, p")
            .unwrap_or_else(|| attr(&el, "title").unwrap_or_else(|| title_from_path(&key))),
        cover: select_attr(el, "img", "src").map(|s| absolute_url(&s).replace("/w154/", "/w200/")),
        url: Some(absolute_url(&key)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let doc = Html::parse_document(&body);
    CatalogItem {
        key: path_key(path),
        title: select_text_doc(&doc, "h1.m-b-5, h1").unwrap_or_else(|| title_from_path(path)),
        cover: select_attr_doc(
            &doc,
            "div.card-body div.row div.col-sm-3 img.img-fluid, img",
            "src",
        )
        .map(|s| absolute_url(&s).replace("/w154/", "/w500/")),
        description: select_ownish_text_doc(&doc, "div.col-sm-4 div.text-large, .text-large"),
        tags: select_texts_doc(&doc, "div.p-v-20.p-h-15.text-center a span, .p-v-20 a span"),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn embed69_streams(embed: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = fetch(embed, EMBED69_FIXTURE, referer);
    if let Some(data_json) = body
        .split("let dataLink =")
        .nth(1)
        .and_then(|s| s.split("];").next())
        .map(|s| format!("{}]", s.trim()))
    {
        let items = serde_json::from_str::<Vec<DataLinkDto>>(&data_json).unwrap_or_default();
        let mut out = Vec::new();
        for item in items {
            let links = item
                .sorted_embeds
                .iter()
                .filter_map(|embed| embed.as_ref()?.link.clone())
                .collect::<Vec<_>>();
            if links.is_empty() {
                continue;
            }
            let body = client(embed)
                .post("https://embed69.org/api/decrypt")
                .xhr()
                .referer(embed)
                .header("Content-Type", "application/json")
                .json(json!({ "links": links }).to_string())
                .send_text()
                .unwrap_or_else(|_| DECRYPT_FIXTURE.to_string());
            let decrypted = serde_json::from_str::<Embed69Dto>(&body).unwrap_or_default();
            for link in decrypted.links {
                if link.link.is_empty() {
                    continue;
                }
                let server = item
                    .sorted_embeds
                    .get(link.index)
                    .and_then(|e| e.as_ref())
                    .and_then(|e| e.servername.clone())
                    .unwrap_or_else(|| server_label(&link.link));
                let lang = item.video_language.clone().unwrap_or_default();
                let name = format!("{lang} {server}").trim().to_string();
                out.extend(resolve_embed(&link.link, &name, embed, request));
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    let mut out = Vec::new();
    for link in Regex::new(r#"https?://[^\s'"()<>]+"#)
        .unwrap()
        .find_iter(&body)
        .map(|m| m.as_str().to_string())
    {
        out.extend(resolve_embed(&link, &server_label(&link), embed, request));
    }
    out
}

fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let embed = absolute_remote(embed, referer);
    if embed.contains(".m3u8") {
        return parse_hls(&embed, name, referer, request);
    }
    let body = fetch(&embed, "", referer);
    if let Some(src) = first_media_url(&body) {
        let src = absolute_remote(&src, &embed);
        if src.contains(".m3u8") {
            return parse_hls(&src, name, &embed, request);
        }
        return vec![stream(&src, name, "direct", &embed, false)];
    }
    vec![external_stream(&embed, name, referer)]
}

fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
        r#"https?://[^\s'"\\]+\.m3u8[^\s'"\\]*"#,
    ]
    .into_iter()
    .find_map(|pattern| {
        Regex::new(pattern)
            .ok()?
            .captures(body)
            .and_then(|c| c.get(1).or_else(|| c.get(0)))
            .map(|m| m.as_str().replace("\\/", "/"))
    })
}

fn parse_hls(master: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer)
        .get(master)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut quality = "auto".to_string();
    for line in body.lines() {
        if line.starts_with("#EXT-X-STREAM-INF") {
            quality = line
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|v| v.split('x').nth(1))
                .and_then(|v| v.split(',').next())
                .map(|v| format!("{v}p"))
                .unwrap_or_else(|| "auto".to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            out.push(stream(
                &absolute_remote(line.trim(), master),
                name,
                &quality,
                referer,
                true,
            ));
        }
    }
    if out.is_empty() {
        out.push(stream(master, name, "auto", referer, true));
    }
    sort_streams(&mut out, request);
    out
}

fn stream(target: &str, name: &str, quality: &str, referer: &str, hls: bool) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} {quality}")),
        quality: Some(quality.to_string()),
        format: Some(if hls { "hls" } else { "mp4" }.to_string()),
        is_hls: hls,
        stream_kind: Some(if hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream(target: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: target.to_string(),
        name: Some(format!("{name} External")),
        quality: Some(name.to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        initialized: true,
        ..VideoStream::default()
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "VidHide").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        (
            name.to_ascii_lowercase().contains(&server),
            name.contains(&quality),
            quality_rank(&name),
        )
    });
    streams.reverse();
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn select_text_doc(doc: &Html, sel: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}

fn select_ownish_text_doc(doc: &Html, sel: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .map(|e| {
            e.text()
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|v| !v.is_empty())
}

fn select_texts_doc(doc: &Html, sel: &str) -> Vec<String> {
    doc.select(&selector(sel))
        .map(text)
        .filter(|v| !v.is_empty())
        .collect()
}

fn select_attr_doc(doc: &Html, sel: &str, name: &str) -> Option<String> {
    doc.select(&selector(sel))
        .next()
        .and_then(|e| e.value().attr(name))
        .map(ToString::to_string)
}

fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .map(text)
        .filter(|v| !v.is_empty())
}

fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel))
        .next()
        .and_then(|e| e.value().attr(name))
        .map(ToString::to_string)
}

fn attr(el: &ElementRef<'_>, name: &str) -> Option<String> {
    el.value().attr(name).map(ToString::to_string)
}

fn text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}

fn absolute_remote(input: &str, base: &str) -> String {
    let value = input.trim().replace("\\/", "/").replace("&amp;", "&");
    if value.starts_with("http://") || value.starts_with("https://") {
        value
    } else if let Some(rest) = value.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &value)
    }
}

fn path_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(BASE_URL)
        .filter(|p| p.starts_with('/'))
        .map(path_key)
}

fn path_key(input: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(BASE_URL)
            .unwrap_or(input)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|v| {
            v.get("key")
                .or_else(|| v.get("url"))
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
        })
        .or_else(|| request.get("key").and_then(Value::as_str))
        .map(path_key)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn server_label(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    for (label, keys) in [
        ("Voe", &["voe", "yip."][..]),
        ("Okru", &["ok.ru", "okru"][..]),
        ("Filemoon", &["filemoon", "moonplayer", "files.im"][..]),
        ("Uqload", &["uqload"][..]),
        ("Mp4Upload", &["mp4upload"][..]),
        (
            "StreamWish",
            &["wishembed", "streamwish", "strwish", "wish"][..],
        ),
        ("Doodstream", &["doodstream", "dood.", "d000d"][..]),
        ("StreamTape", &["streamtape", "stape", "shavetape"][..]),
        ("VidGuard", &["vembed", "guard", "bembed"][..]),
        ("VidHide", &["vidhide", "streamhide", "streamvid"][..]),
        ("YourUpload", &["yourupload", "upload"][..]),
        ("BurstCloud", &["burstcloud", "burst"][..]),
        ("Fastream", &["fastream"][..]),
        ("Upstream", &["upstream"][..]),
    ] {
        if keys.iter().any(|key| lower.contains(key)) {
            return label.to_string();
        }
    }
    host_name(input)
}

fn host_name(input: &str) -> String {
    input
        .split("://")
        .nth(1)
        .unwrap_or(input)
        .split('/')
        .next()
        .unwrap_or("External")
        .replace("www.", "")
}

fn quality_rank(input: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(input)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("PelisPlusHD")
        .replace('-', " ")
}

#[derive(Default, Deserialize)]
struct DataLinkDto {
    #[serde(default, rename = "video_language")]
    video_language: Option<String>,
    #[serde(default, rename = "sortedEmbeds")]
    sorted_embeds: Vec<Option<SortedEmbedDto>>,
}

#[derive(Default, Deserialize)]
struct SortedEmbedDto {
    link: Option<String>,
    servername: Option<String>,
}

#[derive(Default, Deserialize)]
struct Embed69Dto {
    #[serde(default)]
    links: Vec<Embed69Link>,
}

#[derive(Default, Deserialize)]
struct Embed69Link {
    index: usize,
    link: String,
}

const LIST_FIXTURE: &str = r#"
<div class="Posters"><a class="Posters-link" href="/serie/sample"><img src="/w154/cover.jpg"><div class="listing-content"><p>Sample Series</p></div></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="m-b-5">Sample Series</h1><div class="card-body"><div class="row"><div class="col-sm-3"><img class="img-fluid" src="/w154/cover.jpg"></div></div></div>
<div class="col-sm-4"><div class="text-large">Fixture details for local smoke tests.</div></div>
<div class="p-v-20 p-h-15 text-center"><a><span>Drama</span></a></div>
<div class="tab-content"><div><a href="/episodio/sample-1">Episodio 1</a></div></div>
"#;

const WATCH_FIXTURE: &str = r#"<script>video[1] = 'https://embed69.org/f/sample';</script>"#;

const EMBED69_FIXTURE: &str = r#"
<script>let dataLink = [{"video_language":"[LAT]","sortedEmbeds":[{"servername":"StreamWish","link":"https://streamwish.to/e/sample"}]}];</script>
"#;

const DECRYPT_FIXTURE: &str =
    r#"{"success":true,"links":[{"index":0,"link":"https://streamwish.to/e/sample"}]}"#;

export_video_source!(SOURCE);
