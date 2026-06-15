use base64::{Engine, engine::general_purpose};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult, VideoEpisode,
    VideoStream, VideoStreamKind, abi::ExtensionResult, export_video_source, source::VideoSource,
};
use manatan_shared::{
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{Value, json};

const SOURCE: Doramasflix = Doramasflix;
const BASE_URL: &str = "https://doramasflix.in";
const API_URL: &str = "https://sv1.fluxcedene.net/api/gql";
const ACCESS_PLATFORM: &str = "RxARncfg1S_MdpSrCvreoLu_SikCGMzE1NzQzODc3NjE2MQ==";

struct Doramasflix;

impl VideoSource for Doramasflix {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "latest" {
            "CREATEDAT_DESC"
        } else {
            "POPULARITY_DESC"
        };
        Ok(parse_pagination(
            &gql(&list_doramas_query(page(&request), sort), LIST_FIXTURE),
            "paginationDorama",
        ))
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
            return Ok(parse_search(&gql(&search_query(query), SEARCH_FIXTURE)));
        }
        let genre = filter(&request, "genre").unwrap_or_else(|| "doramas".to_string());
        if genre == "peliculas" {
            Ok(parse_pagination(
                &gql(&list_movies_query(page(&request)), LIST_FIXTURE),
                "paginationMovie",
            ))
        } else if genre == "variedades" {
            Ok(parse_pagination(
                &gql(&list_varieties_query(page(&request)), LIST_FIXTURE),
                "paginationDorama",
            ))
        } else {
            self.list(request)
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/doramas-online/sample?id=sample".to_string());
        Ok(fetch_details(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item")
            .unwrap_or_else(|| "/doramas-online/sample?id=sample".to_string());
        if path.contains("/peliculas-online/") {
            return Ok(vec![VideoEpisode {
                key: path.clone(),
                title: Some("Pelicula".to_string()),
                episode_number: Some(1.0),
                url: Some(absolute_url(&path)),
                language: Some("es".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let id = query_param(&path, "id").unwrap_or_default();
        let seasons = gql(&list_seasons_query(&id), SEASONS_FIXTURE);
        let mut episodes = Vec::new();
        for season in seasons
            .pointer("/data/listSeasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let number = season
                .get("season_number")
                .and_then(Value::as_i64)
                .unwrap_or(1);
            let body = gql(&list_episodes_query(&id, number), EPISODES_FIXTURE);
            if let Some(items) = body.pointer("/data/listEpisodes").and_then(Value::as_array) {
                for (idx, ep) in items.iter().enumerate() {
                    let epn = ep
                        .get("episode_number")
                        .and_then(Value::as_f64)
                        .unwrap_or((idx + 1) as f64) as f32;
                    let slug = ep.get("slug").and_then(Value::as_str).unwrap_or_default();
                    let name = ep.get("name").and_then(Value::as_str).unwrap_or_default();
                    let title = if name.is_empty() {
                        format!("T{number} - E{epn}")
                    } else {
                        format!("T{number} - E{epn} - {name}")
                    };
                    let key = format!("/episodios/{slug}");
                    episodes.push(VideoEpisode {
                        key: key.clone(),
                        title: Some(title),
                        episode_number: Some(epn),
                        url: Some(absolute_url(&key)),
                        language: Some("es".to_string()),
                        ..VideoEpisode::default()
                    });
                }
            }
        }
        episodes.reverse();
        Ok(episodes)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let path =
            request_key(&request, "episode").unwrap_or_else(|| "/episodios/sample".to_string());
        let referer = absolute_url(&path);
        let body = fetch(&referer, WATCH_FIXTURE, BASE_URL);
        let state = next_state(&body);
        let mut streams = Vec::new();
        for (embed, lang) in links_online(&state) {
            let real = real_link(&embed).unwrap_or(embed);
            streams.extend(resolve_embed(
                &real,
                &format!("{} {}", lang, host_name(&real)).trim().to_string(),
                &referer,
                &request,
            ));
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
                title: "Doramas populares".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Doramas recientes".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
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
        .with_webview_challenge_fallback()
}
fn api_client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
        .with_header("Origin", BASE_URL)
        .with_header("platform", "doramasflix")
        .with_header("authorization", "Bear")
        .with_header("x-access-platform", ACCESS_PLATFORM)
}
fn fetch(target: &str, fixture: &str, referer: &str) -> String {
    client(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}
fn gql(body: &str, fixture: &str) -> Value {
    let text = api_client()
        .post(API_URL)
        .header("Content-Type", "application/json;charset=UTF-8")
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}
fn parse_pagination(body: &Value, key: &str) -> Paged<CatalogItem> {
    let page = body.pointer(&format!("/data/{key}/pageInfo"));
    let entries = body
        .pointer(&format!("/data/{key}/items"))
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(item_from_json)
        .collect();
    Paged {
        entries,
        has_next_page: page
            .and_then(|p| p.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}
fn parse_search(body: &Value) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for key in ["searchDorama", "searchMovie"] {
        if let Some(items) = body
            .pointer(&format!("/data/{key}"))
            .and_then(Value::as_array)
        {
            entries.extend(items.iter().map(item_from_json));
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}
fn item_from_json(item: &Value) -> CatalogItem {
    let typename = item
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or("Dorama");
    let slug = item.get("slug").and_then(Value::as_str).unwrap_or_default();
    let id = item.get("_id").and_then(Value::as_str).unwrap_or_default();
    let path = url_by_type(typename, slug, id);
    let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
    let name_es = item
        .get("name_es")
        .and_then(Value::as_str)
        .unwrap_or_default();
    CatalogItem {
        key: path.clone(),
        title: if name_es.is_empty() {
            name.to_string()
        } else {
            format!("{name} ({name_es})")
        },
        cover: item
            .get("poster_path")
            .or_else(|| item.get("poster"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(image_url),
        url: Some(absolute_url(&path)),
        description: item
            .get("overview")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .map(|g| {
                g.iter()
                    .filter_map(|x| {
                        x.get("name")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        status: if typename.eq_ignore_ascii_case("Movie") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        initialized: false,
        ..CatalogItem::default()
    }
}
fn fetch_details(path: &str) -> CatalogItem {
    let body = fetch(&absolute_url(path), DETAILS_FIXTURE, BASE_URL);
    let state = next_state(&body);
    if let Some(item) = state.as_object().and_then(|o| {
        o.iter()
            .find(|(k, _)| k.starts_with("Movie:") || k.starts_with("Dorama:"))
            .map(|(_, v)| v)
    }) {
        let mut out = item_from_json(item);
        out.initialized = true;
        return out;
    }
    CatalogItem {
        key: path_key(path),
        title: title_from_path(path),
        url: Some(absolute_url(path)),
        language: Some("es".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}
fn next_state(body: &str) -> Value {
    let doc = Html::parse_document(body);
    for script in doc.select(&Selector::parse("script").unwrap()) {
        let data = script.text().collect::<Vec<_>>().join("");
        if data.contains("{\"props\":{\"pageProps\":{") {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                return v
                    .pointer("/props/pageProps/apolloState")
                    .cloned()
                    .unwrap_or(v);
            }
        }
    }
    Value::Null
}
fn links_online(state: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    fn scan(value: &Value, out: &mut Vec<(String, String)>) {
        match value {
            Value::Object(map) => {
                if let Some(items) = map
                    .get("links_online")
                    .and_then(|v| v.get("json"))
                    .and_then(Value::as_array)
                {
                    for item in items {
                        if let Some(link) = item.get("link").and_then(Value::as_str) {
                            out.push((
                                link.to_string(),
                                item.get("lang")
                                    .and_then(Value::as_str)
                                    .map(lang_label)
                                    .unwrap_or_default(),
                            ));
                        }
                    }
                }
                if let Some(server) = map.get("server").and_then(|v| v.get("json")) {
                    if let Some(link) = server.get("link").and_then(Value::as_str) {
                        out.push((
                            link.to_string(),
                            server
                                .get("lang")
                                .and_then(Value::as_str)
                                .map(lang_label)
                                .unwrap_or_default(),
                        ));
                    }
                }
                for v in map.values() {
                    scan(v, out);
                }
            }
            Value::Array(items) => {
                for v in items {
                    scan(v, out);
                }
            }
            _ => {}
        }
    }
    scan(state, &mut out);
    out.sort();
    out.dedup();
    out
}
fn real_link(link: &str) -> Option<String> {
    if !link.contains("fkplayer.xyz") {
        return Some(link.to_string());
    }
    let body = fetch(link, "", BASE_URL);
    let state = next_state(&body);
    let token = state
        .pointer("/props/pageProps/token")
        .or_else(|| state.pointer("/query/token"))
        .and_then(Value::as_str)?;
    let response = client(link)
        .post("https://fkplayer.xyz/api/decoding")
        .header("Origin", "https://fkplayer.xyz")
        .header("Content-Type", "application/json")
        .json(json!({ "token": token }).to_string())
        .send_text()
        .ok()?;
    let encoded = serde_json::from_str::<Value>(&response)
        .ok()?
        .get("link")?
        .as_str()?
        .to_string();
    general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}
fn resolve_embed(embed: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if embed.contains(".m3u8") {
        return parse_hls(embed, name, referer, request);
    }
    let body = fetch(embed, "", referer);
    if let Some(src) = first_media_url(&body).map(|s| absolute_remote(&s, embed)) {
        if src.contains(".m3u8") {
            parse_hls(&src, name, embed, request)
        } else {
            vec![stream(&src, name, "direct", embed, false)]
        }
    } else {
        vec![external_stream(embed, name, referer)]
    }
}
fn first_media_url(body: &str) -> Option<String> {
    [
        r#"file\s*:\s*["']([^"']+)["']"#,
        r#"src\s*:\s*["']([^"']+)["']"#,
        r#"<source[^>]+src=["']([^"']+)["']"#,
    ]
    .into_iter()
    .find_map(|p| {
        Regex::new(p)
            .ok()?
            .captures(body)?
            .get(1)
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
        ..VideoStream::default()
    }
}
fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let server = pref(request, "preferred_server", "Voe").to_ascii_lowercase();
    let quality = pref(request, "preferred_quality", "1080");
    let lang = pref(request, "preferred_language", "[LAT]");
    streams.sort_by_key(|s| {
        let name = s.name.clone().unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let q = s.quality.clone().unwrap_or_default();
        (
            name.contains(&lang),
            lower.contains(&server),
            q.contains(&quality),
            quality_rank(&q),
        )
    });
    streams.reverse();
}
fn list_doramas_query(page: u64, sort: &str) -> String {
    json!({"operationName":"listDoramas","variables":{"page":page,"sort":sort,"perPage":32,"filter":{"isTVShow":false}},"query":"query listDoramas($page: Int, $perPage: Int, $sort: SortFindManyDoramaInput, $filter: FilterFindManyDoramaInput) { paginationDorama(page: $page, perPage: $perPage, sort: $sort, filter: $filter) { pageInfo { hasNextPage } items { _id name name_es slug overview poster_path poster __typename genres { name slug __typename } } } }"}).to_string()
}
fn list_movies_query(page: u64) -> String {
    json!({"operationName":"listMovies","variables":{"page":page,"perPage":32,"sort":"CREATEDAT_DESC","filter":{}},"query":"query listMovies($page: Int, $perPage: Int, $sort: SortFindManyMovieInput, $filter: FilterFindManyMovieInput) { paginationMovie(page: $page, perPage: $perPage, sort: $sort, filter: $filter) { pageInfo { hasNextPage } items { _id name name_es slug overview poster_path poster __typename genres { name __typename } } } }"}).to_string()
}
fn list_varieties_query(page: u64) -> String {
    json!({"operationName":"listDoramas","variables":{"page":page,"sort":"CREATEDAT_DESC","perPage":32,"filter":{"isTVShow":true}},"query":"query listDoramas($page: Int, $perPage: Int, $sort: SortFindManyDoramaInput, $filter: FilterFindManyDoramaInput) { paginationDorama(page: $page, perPage: $perPage, sort: $sort, filter: $filter) { pageInfo { hasNextPage } items { _id name name_es slug overview poster_path poster __typename genres { name slug __typename } } } }"}).to_string()
}
fn search_query(input: &str) -> String {
    json!({"operationName":"searchAll","variables":{"input":input},"query":"query searchAll($input: String!) { searchDorama(input: $input, limit: 32) { _id slug name name_es poster_path poster __typename } searchMovie(input: $input, limit: 32) { _id name name_es slug poster_path poster __typename } }"}).to_string()
}
fn list_seasons_query(id: &str) -> String {
    json!({"operationName":"listSeasons","variables":{"serie_id":id},"query":"query listSeasons($serie_id: MongoID!) { listSeasons(sort: NUMBER_ASC, filter: {serie_id: $serie_id}) { slug season_number poster_path air_date serie_name poster __typename } }"}).to_string()
}
fn list_episodes_query(id: &str, season: i64) -> String {
    json!({"operationName":"listEpisodes","variables":{"serie_id":id,"season_number":season},"query":"query listEpisodes($season_number: Float!, $serie_id: MongoID!) { listEpisodes(sort: NUMBER_ASC, filter: {type_serie: \"dorama\", serie_id: $serie_id, season_number: $season_number}) { _id name slug serie_name serie_name_es air_date season_number episode_number poster __typename } }"}).to_string()
}
fn url_by_type(typename: &str, slug: &str, id: &str) -> String {
    if typename.eq_ignore_ascii_case("Movie") {
        format!("/peliculas-online/{slug}?id={id}")
    } else {
        format!("/doramas-online/{slug}?id={id}")
    }
}
fn image_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        format!("https://image.tmdb.org/t/p/w220_and_h330_face{input}")
    }
}
fn absolute_url(input: &str) -> String {
    absolute_remote(input, BASE_URL)
}
fn absolute_remote(input: &str, base: &str) -> String {
    let t = input.trim().replace("\\/", "/");
    if t.starts_with("http") {
        t
    } else if let Some(rest) = t.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url::join_url(base, &t)
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
            .split('#')
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
fn query_param(path: &str, name: &str) -> Option<String> {
    path.split('?').nth(1)?.split('&').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
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
    json!({ "listing": listing, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null) })
}
fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
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
fn title_from_path(path: &str) -> String {
    path.trim_matches('/')
        .split('?')
        .next()
        .unwrap_or(path)
        .rsplit('/')
        .next()
        .unwrap_or("Doramasflix")
        .replace('-', " ")
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
fn quality_rank(q: &str) -> i32 {
    Regex::new(r#"(\d+)"#)
        .unwrap()
        .captures(q)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}
fn lang_label(id: &str) -> String {
    match id {
        "36" => "[ENG]",
        "37" => "[CAST]",
        "38" => "[LAT]",
        "192" => "[SUB]",
        "1327" => "[POR]",
        "13109" => "[COR]",
        "13110" => "[JAP]",
        _ => "",
    }
    .to_string()
}
fn referer_headers(referer: &str) -> Context {
    let mut h = Context::new();
    h.insert("Referer".to_string(), referer.to_string());
    h
}

const LIST_FIXTURE: &str = r#"{"data":{"paginationDorama":{"pageInfo":{"hasNextPage":false},"items":[{"_id":"sample","name":"Sample","name_es":"Muestra","slug":"sample","poster_path":"/poster.jpg","__typename":"Dorama","genres":[]}]}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"searchDorama":[],"searchMovie":[]}}"#;
const SEASONS_FIXTURE: &str =
    r#"{"data":{"listSeasons":[{"slug":"sample","season_number":1,"__typename":"Season"}]}}"#;
const EPISODES_FIXTURE: &str = r#"{"data":{"listEpisodes":[{"_id":"sample","name":"Sample","slug":"sample-1","season_number":1,"episode_number":1,"__typename":"Episode"}]}}"#;
const DETAILS_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"apolloState":{"Dorama:sample":{"_id":"sample","name":"Sample","name_es":"Muestra","slug":"sample","poster_path":"/poster.jpg","__typename":"Dorama"}}}}}</script>"#;
const WATCH_FIXTURE: &str = r#"<script>{"props":{"pageProps":{"apolloState":{"Episode:sample":{"links_online":{"json":[{"link":"https://example.invalid/embed","lang":"38"}]}}}}}}</script>"#;

export_video_source!(SOURCE);
