use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::Value;
use std::marker::PhantomData;

pub trait PtVideoConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "safe";
    const POPULAR_TITLE: &'static str = "Popular";
    const LATEST_TITLE: &'static str = "Lancamentos";
    const LIST_SELECTOR: &'static str;
    const LATEST_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const SEARCH_SELECTOR: &'static str = Self::LIST_SELECTOR;
    const DETAILS_TITLE_SELECTOR: &'static str = "h1";
    const DETAILS_COVER_SELECTOR: &'static str = "img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "p";
    const DETAILS_TAG_SELECTOR: &'static str = "a[rel='tag'], .genres a, .sgeneros a";
    const EPISODE_SELECTOR: &'static str;
    const EPISODE_TITLE_SELECTOR: &'static str = "";
    const EPISODE_NUMBER_SELECTOR: &'static str = "";
    const PLAYER_SELECTOR: &'static str = "source[src], iframe[src], script";
    const USE_DOO_AJAX: bool = false;
    const DOO_ENDPOINT: DooEndpoint = DooEndpoint::AdminAjax;
    const RESOLVE_EMBED_PAGE: bool = true;

    fn popular_url(page: u64) -> String;
    fn latest_url(page: u64) -> String;
    fn search_url(page: u64, query: &str, request: &Value) -> String;
    fn search_override(_page: u64, _query: &str, _request: &Value) -> Option<Paged<CatalogItem>> {
        None
    }
    fn real_details_url(path: &str, _body: &str) -> Option<String> {
        let _ = path;
        None
    }
    fn normalize_item_path(href: &str) -> String {
        path_key::<Self>(href)
    }
    fn card_title(el: ElementRef<'_>, path: &str) -> String {
        attr(&el, "title")
            .if_empty(&select_attr(el, "img", "alt").unwrap_or_default())
            .if_empty(&select_text(el, "h1, h2, h3, h4, .title, .titulo, .name").unwrap_or_default())
            .if_empty(&title_from_path::<Self>(path))
    }
    fn card_cover(el: ElementRef<'_>) -> Option<String> {
        image_from(el).map(|src| absolute_url::<Self>(&src))
    }
    fn episode_from_element(el: ElementRef<'_>) -> Option<VideoEpisode>
    where
        Self: Sized,
    {
        default_episode_from_element::<Self>(el)
    }
    fn streams_from_page(body: &str, referer: &str, request: &Value) -> Vec<VideoStream>
    where
        Self: Sized,
    {
        default_streams_from_page::<Self>(body, referer, request)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DooEndpoint {
    AdminAjax,
    WpJsonV1,
    WpJsonV2,
}

pub struct PtVideoSource<C>(PhantomData<C>);

impl<C> PtVideoSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: PtVideoConfig> VideoSource for PtVideoSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if listing(&request) == "latest" {
            C::latest_url(page(&request))
        } else {
            C::popular_url(page(&request))
        };
        let selector = if listing(&request) == "latest" {
            C::LATEST_SELECTOR
        } else {
            C::LIST_SELECTOR
        };
        Ok(parse_cards::<C>(&fetch::<C>(&target, LIST_FIXTURE, C::BASE_URL), selector))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(path) = path_from_url::<C>(query) {
            return Ok(Paged {
                entries: vec![fetch_details::<C>(&path)],
                has_next_page: false,
            });
        }
        if let Some(result) = C::search_override(page(&request), query, &request) {
            return Ok(result);
        }
        let target = C::search_url(page(&request), query, &request);
        Ok(parse_cards::<C>(
            &fetch::<C>(&target, LIST_FIXTURE, C::BASE_URL),
            C::SEARCH_SELECTOR,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key::<C>(&request, "item").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details::<C>(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key::<C>(&request, "item").unwrap_or_else(|| "/sample".to_string());
        let body = fetch::<C>(&absolute_url::<C>(&path), DETAILS_FIXTURE, C::BASE_URL);
        let body = if let Some(url) = C::real_details_url(&path, &body) {
            fetch::<C>(&url, &body, C::BASE_URL)
        } else {
            body
        };
        let doc = Html::parse_document(&body);
        Ok(doc
            .select(&selector(C::EPISODE_SELECTOR))
            .filter_map(C::episode_from_element)
            .collect())
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key::<C>(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let referer = absolute_url::<C>(&episode);
        let body = fetch::<C>(&referer, PLAYER_FIXTURE, C::BASE_URL);
        let mut streams = C::streams_from_page(&body, &referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(with_listing(&request, "popular"))?;
        let latest = self.list(with_listing(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: C::POPULAR_TITLE.to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: C::LATEST_TITLE.to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key::<C>(&request, "item").map(|path| absolute_url::<C>(&path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key::<C>(&request, "episode").map(|path| absolute_url::<C>(&path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(path) = path_from_url::<C>(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details::<C>(&path)),
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

pub fn client<C: PtVideoConfig>(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Origin", C::BASE_URL)
        .with_header("Accept-Language", "pt-BR,pt;q=0.9,en-US;q=0.8,en;q=0.7")
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback()
}

pub fn fetch<C: PtVideoConfig>(target: &str, fixture: &str, referer: &str) -> String {
    client::<C>(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

pub fn parse_cards<C: PtVideoConfig>(body: &str, sel: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut entries = Vec::new();
    for el in doc.select(&selector(sel)) {
        if let Some(item) = card_from_element::<C>(el) {
            if !entries.iter().any(|existing: &CatalogItem| existing.key == item.key) {
                entries.push(item);
            }
        }
    }
    Paged {
        entries,
        has_next_page: doc
            .select(&selector("a.next, li.next, .pagination a[rel='next'], div.pagination a:last-child, ul.content-pagination li.next"))
            .next()
            .is_some(),
    }
}

fn card_from_element<C: PtVideoConfig>(el: ElementRef<'_>) -> Option<CatalogItem> {
    let anchor = if el.value().name() == "a" {
        el
    } else {
        el.select(&selector("a[href]")).next().unwrap_or(el)
    };
    let href = attr(&anchor, "href");
    if href.is_empty() || href.contains("wp-admin") || href.ends_with(".jpg") || href.ends_with(".png") {
        return None;
    }
    let path = C::normalize_item_path(&href);
    Some(CatalogItem {
        key: path.clone(),
        title: C::card_title(el, &path),
        cover: C::card_cover(el),
        url: Some(absolute_url::<C>(&path)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

pub fn fetch_details<C: PtVideoConfig>(path: &str) -> CatalogItem {
    let initial = fetch::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
    let body = if let Some(url) = C::real_details_url(path, &initial) {
        fetch::<C>(&url, &initial, C::BASE_URL)
    } else {
        initial
    };
    let doc = Html::parse_document(&body);
    let root = doc.root_element();
    let title = select_text(root, C::DETAILS_TITLE_SELECTOR).unwrap_or_else(|| title_from_path::<C>(path));
    let status_text = select_text(root, ".status, .anime_status, li:contains('Status'), div:contains('Status')").unwrap_or_default();
    CatalogItem {
        key: path_key::<C>(path),
        title,
        cover: root
            .select(&selector(C::DETAILS_COVER_SELECTOR))
            .next()
            .and_then(image_from)
            .map(|src| absolute_url::<C>(&src)),
        description: select_text(root, C::DETAILS_DESCRIPTION_SELECTOR),
        tags: root
            .select(&selector(C::DETAILS_TAG_SELECTOR))
            .map(text)
            .filter(|v| !v.is_empty())
            .collect(),
        url: Some(absolute_url::<C>(path)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        status: parse_status(&status_text),
        initialized: true,
        ..CatalogItem::default()
    }
}

pub fn default_episode_from_element<C: PtVideoConfig>(el: ElementRef<'_>) -> Option<VideoEpisode> {
    let anchor = if el.value().name() == "a" {
        el
    } else {
        el.select(&selector("a[href]")).next().unwrap_or(el)
    };
    let href = attr(&anchor, "href");
    if href.is_empty() {
        return None;
    }
    let raw_title = if C::EPISODE_TITLE_SELECTOR.is_empty() {
        text(el)
    } else {
        select_text(el, C::EPISODE_TITLE_SELECTOR).unwrap_or_else(|| text(el))
    };
    let number_text = if C::EPISODE_NUMBER_SELECTOR.is_empty() {
        raw_title.clone()
    } else {
        select_text(el, C::EPISODE_NUMBER_SELECTOR).unwrap_or_else(|| raw_title.clone())
    };
    let number = first_number(&number_text).unwrap_or_else(|| first_number(&raw_title).unwrap_or(1.0));
    let key = path_key::<C>(&href);
    Some(VideoEpisode {
        key: key.clone(),
        title: Some(raw_title.if_empty(&format!("Episode {}", display_number(number)))),
        episode_number: Some(number),
        url: Some(absolute_url::<C>(&key)),
        language: Some(C::LANG.to_string()),
        ..VideoEpisode::default()
    })
}

pub fn default_streams_from_page<C: PtVideoConfig>(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let mut out = Vec::new();
    let doc = Html::parse_document(body);
    if C::USE_DOO_AJAX {
        for player in doc.select(&selector("ul#playeroptionsul li, li.dooplay_player_option, li.player-option")) {
            if let Some(embed) = doo_embed::<C>(player, referer) {
                let name = select_text(player, "span.title, .title").unwrap_or_else(|| "Player".to_string());
                out.extend(resolve_embed::<C>(&embed, &name, referer, request, 0));
            }
        }
    }
    for el in doc.select(&selector(C::PLAYER_SELECTOR)) {
        out.extend(streams_from_element::<C>(el, referer, "Default", request, 0));
    }
    out
}

pub fn doo_embed<C: PtVideoConfig>(player: ElementRef<'_>, referer: &str) -> Option<String> {
    let post = attr(&player, "data-post");
    let nume = attr(&player, "data-nume");
    let kind = attr(&player, "data-type").if_empty("movie");
    if post.is_empty() || nume.is_empty() {
        return None;
    }
    let body = match C::DOO_ENDPOINT {
        DooEndpoint::AdminAjax => client::<C>(referer)
            .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
            .xhr()
            .form(&[
                ("action", "doo_player_ajax"),
                ("post", post.as_str()),
                ("nume", nume.as_str()),
                ("type", kind.as_str()),
            ])
            .send_text(),
        DooEndpoint::WpJsonV1 => client::<C>(referer)
            .get(format!("{}/wp-json/dooplayer/v1/post/{post}?type={kind}&source={nume}", C::BASE_URL))
            .xhr()
            .send_text(),
        DooEndpoint::WpJsonV2 => client::<C>(referer)
            .get(format!("{}/wp-json/dooplayer/v2/{post}/{kind}/{nume}", C::BASE_URL))
            .xhr()
            .send_text(),
    }
    .unwrap_or_else(|_| EMBED_RESPONSE_FIXTURE.to_string());
    serde_json::from_str::<EmbedResponse>(&body)
        .ok()
        .map(|res| absolute_remote(&res.embed_url.replace("\\/", "/").replace('\\', ""), C::BASE_URL))
}

pub fn streams_from_element<C: PtVideoConfig>(
    el: ElementRef<'_>,
    referer: &str,
    name: &str,
    request: &Value,
    depth: usize,
) -> Vec<VideoStream> {
    if depth > 3 {
        return Vec::new();
    }
    match el.value().name() {
        "source" | "video" => {
            let src = attr(&el, "src").if_empty(&select_attr(el, "source", "src").unwrap_or_default());
            if src.is_empty() {
                return Vec::new();
            }
            vec![stream_for_url::<C>(&absolute_remote(&src, referer), name, referer, request)]
        }
        "iframe" => {
            let src = attr(&el, "data-src").if_empty(&attr(&el, "src"));
            resolve_embed::<C>(&absolute_remote(&src, referer), name, referer, request, depth + 1)
        }
        "script" => streams_from_script::<C>(&text_or_data(el), referer, name, request),
        "a" => {
            let href = attr(&el, "href");
            if href.is_empty() {
                Vec::new()
            } else {
                vec![stream_for_url::<C>(&absolute_remote(&href, referer), name, referer, request)]
            }
        }
        _ => Vec::new(),
    }
}

pub fn resolve_embed<C: PtVideoConfig>(
    embed: &str,
    name: &str,
    referer: &str,
    request: &Value,
    depth: usize,
) -> Vec<VideoStream> {
    if embed.is_empty() {
        return Vec::new();
    }
    if embed.contains(".m3u8") || embed.contains(".mp4") || embed.starts_with("magnet:") || embed.ends_with(".torrent") {
        return vec![stream_for_url::<C>(embed, name, referer, request)];
    }
    if !C::RESOLVE_EMBED_PAGE || depth > 3 {
        return vec![external_stream(embed, name, referer)];
    }
    let body = fetch::<C>(embed, "", referer);
    let doc = Html::parse_document(&body);
    let mut out = Vec::new();
    for el in doc.select(&selector("video[src], source[src], iframe[src], script")) {
        out.extend(streams_from_element::<C>(el, embed, name, request, depth + 1));
    }
    if out.is_empty() {
        out.extend(streams_from_script::<C>(&body, embed, name, request));
    }
    if out.is_empty() {
        out.push(external_stream(embed, name, referer));
    }
    out
}

pub fn streams_from_script<C: PtVideoConfig>(script: &str, referer: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let cleaned = script
        .replace("\\/", "/")
        .replace("\\\"", "\"")
        .replace("\\'", "'");
    let re = Regex::new(r#"(?s)(?:file|src|source)\s*[:=]\s*["']([^"']+)["'](?:[^{}]+?(?:label|res|size)\s*[:=]\s*["']?([^"',}]+)["']?)?|(?:label|res|size)\s*[:=]\s*["']?([^"',}]+)["']?[^{}]+?(?:file|src|source)\s*[:=]\s*["']([^"']+)["']"#).unwrap();
    let mut out = Vec::new();
    for caps in re.captures_iter(&cleaned) {
        let src = caps.get(1).or_else(|| caps.get(4)).map(|m| m.as_str()).unwrap_or_default();
        if src.is_empty() || !(src.starts_with("http") || src.starts_with("//") || src.contains(".m3u8") || src.contains(".mp4")) {
            continue;
        }
        let quality = caps.get(2).or_else(|| caps.get(3)).map(|m| m.as_str()).unwrap_or(name);
        out.push(stream_for_url::<C>(&absolute_remote(src, referer), quality, referer, request));
    }
    if out.is_empty() {
        for src in cleaned
            .split(['"', '\'', '`'])
            .filter(|part| part.contains(".m3u8") || part.contains(".mp4"))
        {
            out.push(stream_for_url::<C>(&absolute_remote(src, referer), name, referer, request));
        }
    }
    out
}

pub fn stream_for_url<C: PtVideoConfig>(src: &str, name: &str, referer: &str, request: &Value) -> VideoStream {
    let is_hls = src.contains(".m3u8");
    let is_external = !(src.contains(".m3u8") || src.contains(".mp4") || src.starts_with("magnet:") || src.ends_with(".torrent"));
    let quality = quality_from(src)
        .or_else(|| quality_from(name))
        .unwrap_or_else(|| preference(request, "preferred_quality", "720p"));
    VideoStream {
        url: src.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.clone()),
        format: Some(if is_hls { "hls" } else if is_external { "external" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else if is_external {
            VideoStreamKind::External
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        preferred: quality == preference(request, "preferred_quality", "720p"),
        initialized: true,
        ..VideoStream::default()
    }
}

pub fn external_stream(src: &str, name: &str, referer: &str) -> VideoStream {
    VideoStream {
        url: src.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        headers: referer_headers(referer),
        preferred: true,
        initialized: true,
        ..VideoStream::default()
    }
}

pub fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred_quality = preference(request, "preferred_quality", "720p");
    let preferred_language = preference(request, "preferred_language", "");
    streams.sort_by_key(|stream| {
        let name = stream.name.as_deref().unwrap_or_default().to_lowercase();
        let quality = stream.quality.as_deref().unwrap_or_default();
        (
            name.contains(&preferred_language.to_lowercase()),
            quality == preferred_quality,
            quality_score(quality),
        )
    });
    streams.reverse();
}

pub fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

pub fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

pub fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

pub fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

pub fn with_listing(request: &Value, list: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(list.to_string()));
    }
    cloned
}

pub fn request_key<C: PtVideoConfig>(request: &Value, field: &str) -> Option<String> {
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
        .map(path_key::<C>)
}

pub fn path_from_url<C: PtVideoConfig>(input: &str) -> Option<String> {
    (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
}

pub fn path_key<C: PtVideoConfig + ?Sized>(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
    format!("/{}", without_base.split('#').next().unwrap_or(without_base).trim_matches('/'))
}

pub fn absolute_url<C: PtVideoConfig + ?Sized>(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(C::BASE_URL, input)
    }
}

pub fn absolute_remote(input: &str, base: &str) -> String {
    if input.starts_with("http") || input.starts_with("magnet:") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!("{}/{}", root.trim_end_matches('/'), input.trim_start_matches('/'))
    }
}

pub fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

pub fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
}

pub fn text(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn text_or_data(el: ElementRef<'_>) -> String {
    let data = el
        .children()
        .filter_map(|child| child.value().as_text())
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if data.is_empty() { text(el) } else { data }
}

pub fn select_text(el: ElementRef<'_>, sel: &str) -> Option<String> {
    el.select(&selector(sel)).next().map(text).filter(|v| !v.is_empty())
}

pub fn select_attr(el: ElementRef<'_>, sel: &str, name: &str) -> Option<String> {
    el.select(&selector(sel)).next().map(|e| attr(&e, name)).filter(|v| !v.is_empty())
}

pub fn image_from(el: ElementRef<'_>) -> Option<String> {
    if el.value().name() == "img" {
        return attr(&el, "data-src")
            .if_empty(&attr(&el, "data-lazy-src"))
            .if_empty(&attr(&el, "src"))
            .if_empty(&attr(&el, "style"))
            .extract_image();
    }
    select_attr(el, "img", "data-src")
        .or_else(|| select_attr(el, "img", "data-lazy-src"))
        .or_else(|| select_attr(el, "img", "data-wpfc-original-src"))
        .or_else(|| select_attr(el, "img", "src"))
        .or_else(|| select_attr(el, "[style*='background-image']", "style").and_then(|v| v.extract_image()))
        .filter(|v| !v.is_empty())
}

pub fn title_from_path<C: PtVideoConfig + ?Sized>(path: &str) -> String {
    path.trim_matches('/').rsplit('/').next().unwrap_or(C::NAME).replace('-', " ")
}

pub fn first_number(input: &str) -> Option<f32> {
    Regex::new(r"\d+(?:[\.,]\d+)?")
        .ok()?
        .find(input)?
        .as_str()
        .replace(',', ".")
        .parse()
        .ok()
}

pub fn display_number(value: f32) -> String {
    if value.fract() == 0.0 { format!("{}", value as u32) } else { value.to_string() }
}

fn parse_status(input: &str) -> ItemStatus {
    let value = normalize(input);
    if value.contains("completo") {
        ItemStatus::Completed
    } else if value.contains("progresso") || value.contains("lancamento") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .replace(['á', 'à', 'ã', 'â'], "a")
        .replace(['é', 'ê'], "e")
        .replace('í', "i")
        .replace(['ó', 'õ', 'ô'], "o")
        .replace('ú', "u")
        .replace('ç', "c")
}

pub fn quality_from(input: &str) -> Option<String> {
    Regex::new(r"(?i)(\d{3,4}p|full\s*hd|fhd|hd|sd)")
        .ok()?
        .find(input)
        .map(|m| match m.as_str().to_lowercase().replace(' ', "").as_str() {
            "fullhd" | "fhd" => "1080p".to_string(),
            "hd" => "720p".to_string(),
            "sd" => "480p".to_string(),
            value => value.to_string(),
        })
}

pub fn quality_score(input: &str) -> u32 {
    first_number(input).map(|v| v as u32).unwrap_or(0)
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
    fn extract_image(self) -> Option<String>;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() { fallback.to_string() } else { self }
    }

    fn extract_image(self) -> Option<String> {
        let value = self.trim();
        if value.is_empty() {
            None
        } else if value.contains("url(") {
            Some(value.substring_between("url('", "')").or_else(|| value.substring_between("url(\"", "\")")).unwrap_or_else(|| {
                value.trim_start_matches("background-image:")
                    .trim()
                    .trim_start_matches("url(")
                    .trim_end_matches(')')
                    .trim_matches(['"', '\''])
                    .to_string()
            }))
        } else {
            Some(value.to_string())
        }
    }
}

trait Between {
    fn substring_between(&self, start: &str, end: &str) -> Option<String>;
}

impl Between for str {
    fn substring_between(&self, start: &str, end: &str) -> Option<String> {
        Some(self.split(start).nth(1)?.split(end).next()?.to_string())
    }
}

#[derive(Deserialize)]
struct EmbedResponse {
    embed_url: String,
}

const LIST_FIXTURE: &str = r#"<div class="item"><a href="/sample"><img alt="Sample" src="/poster.jpg"><h3>Sample</h3></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><img src="/poster.jpg"><p>Sample details.</p><a href="/sample-1">Episode 1</a>"#;
const PLAYER_FIXTURE: &str = r#"<video><source src="https://example.invalid/video.mp4" label="720p"></video>"#;
const EMBED_RESPONSE_FIXTURE: &str = r#"{"embed_url":"https://example.invalid/embed"}"#;
