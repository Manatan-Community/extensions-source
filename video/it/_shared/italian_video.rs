use manatan_extension::{
    AudioTrack, CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, Paged, UrlResolveResult,
    VideoEpisode, VideoHoster, VideoStream, VideoStreamKind,
    abi::{ExtensionError, ExtensionResult},
};
use manatan_shared::{
    html,
    sdk::{Context, SearchRequest, http::HttpClient},
    url,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub trait SaturnConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const CONTENT_RATING: &'static str;
    const LIST_PATH: &'static str;
    const LATEST_PATH: &'static str;
    const ARCHIVE_PATH: &'static str;
    const CARD_IMG_CLASS: &'static str;
    const TITLE_CLASS: &'static str;
}

pub struct SaturnSource<C>(std::marker::PhantomData<C>);

impl<C> SaturnSource<C> {
    pub const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<C: SaturnConfig> manatan_extension::source::VideoSource for SaturnSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{}{path}?page={page}", C::BASE_URL, path = C::LATEST_PATH)
        } else {
            format!("{}{path}?page={page}", C::BASE_URL, path = C::LIST_PATH)
        };
        let body = fetch(C::BASE_URL, &target, SATURN_LIST_FIXTURE);
        Ok(Paged {
            entries: saturn_cards::<C>(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(path) = path_from_url(C::BASE_URL, &query) {
            return Ok(Paged {
                entries: vec![saturn_details::<C>(&path)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let params = filter_params(&request);
        let target = if params.is_empty() {
            format!(
                "{}{}?search={}",
                C::BASE_URL,
                C::ARCHIVE_PATH,
                url::query_escape(&query)
            )
        } else {
            format!("{}{path}?{params}&page={page}", C::BASE_URL, path = "/filter")
        };
        let body = fetch(C::BASE_URL, &target, SATURN_LIST_FIXTURE);
        Ok(Paged {
            entries: saturn_cards::<C>(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        Ok(saturn_details::<C>(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_else(|| "/anime/sample".to_string());
        let body = fetch(C::BASE_URL, &absolute(C::BASE_URL, &path), SATURN_DETAILS_FIXTURE);
        let doc = Html::parse_document(&body);
        let mut out = select(&doc, "div.btn-group.episodes-button.episodi-link-button a")
            .into_iter()
            .filter_map(|a| {
                let href = attr(&a, "href");
                if href.is_empty() {
                    return None;
                }
                let title = text(&a);
                let number = first_number(&title).unwrap_or(1.0);
                Some(VideoEpisode {
                    key: path_key(C::BASE_URL, &href),
                    title: Some(title),
                    episode_number: Some(number),
                    url: Some(absolute(C::BASE_URL, &href)),
                    language: Some("it".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode").unwrap_or_default();
        Ok(vec![VideoHoster {
            key: episode.clone(),
            name: C::NAME.to_string(),
            url: Some(absolute(C::BASE_URL, &episode)),
            lazy: true,
            video_count: Some(1),
            headers: referer_headers(C::BASE_URL),
            ..VideoHoster::default()
        }])
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "hoster").unwrap_or_default();
        let episode_url = absolute(C::BASE_URL, &episode);
        let episode_body = fetch(C::BASE_URL, &episode_url, SATURN_EPISODE_FIXTURE);
        let watch =
            first_href_containing(&episode_body, "/watch").unwrap_or_else(|| episode_url.clone());
        let watch_url = format!("{}&s=alt", absolute(C::BASE_URL, &watch));
        let body = fetch(&episode_url, &watch_url, SATURN_WATCH_FIXTURE);
        let mut streams = streams_from_player(&body, &watch_url, C::NAME, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        streams_from_hosters(self, request)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        home_sections(self, request, "Popolari", "Ultimi aggiornamenti")
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute(C::BASE_URL, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute(C::BASE_URL, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        handle_url(C::BASE_URL, &request, |path| saturn_details::<C>(&path))
    }
}

pub struct AnimeWorldSource;

impl manatan_extension::source::VideoSource for AnimeWorldSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let target = if listing(&request) == "latest" {
            format!("{ANIMEWORLD}/updated?page={page}")
        } else {
            format!("{ANIMEWORLD}/filter?sort=6&page={page}")
        };
        Ok(animeworld_listing(&fetch(ANIMEWORLD, &target, ANIMEWORLD_LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(path) = path_from_url(ANIMEWORLD, &query) {
            return Ok(Paged {
                entries: vec![animeworld_details(&path)],
                has_next_page: false,
            });
        }
        let mut params = animeworld_filter_params(&request);
        if !query.is_empty() {
            params.push_str("&keyword=");
            params.push_str(&url::query_escape(&query));
        }
        let target = format!("{ANIMEWORLD}/filter?{params}&page={}", page(&request));
        Ok(animeworld_listing(&fetch(
            ANIMEWORLD,
            &target,
            ANIMEWORLD_LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(animeworld_details(
            &request_key(&request, "item").unwrap_or_default(),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_default();
        let body = fetch(ANIMEWORLD, &absolute(ANIMEWORLD, &path), ANIMEWORLD_DETAILS_FIXTURE);
        let doc = Html::parse_document(&body);
        let mut out = select(&doc, "div.server.active ul.episodes li.episode a")
            .into_iter()
            .filter_map(|a| {
                let href = attr(&a, "href");
                if href.is_empty() {
                    return None;
                }
                let title = format!("Episode: {}", text(&a));
                Some(VideoEpisode {
                    key: path_key(ANIMEWORLD, &href),
                    title: Some(title.clone()),
                    episode_number: first_number(&title),
                    url: Some(absolute(ANIMEWORLD, &href)),
                    language: Some("it".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode").unwrap_or_default();
        let episode_url = absolute(ANIMEWORLD, &episode);
        let body = fetch(ANIMEWORLD, &episode_url, ANIMEWORLD_EPISODE_FIXTURE);
        let doc = Html::parse_document(&body);
        let ep_id = select(&doc, "div#player[data-episode-id]")
            .first()
            .map(|el| attr(el, "data-episode-id"))
            .unwrap_or_default();
        let mut out = Vec::new();
        for tab in select(&doc, "div.servers > div.widget-title span.server-tab") {
            let name = text(&tab);
            let server_name = attr(&tab, "data-name");
            let selector = format!("div.server[data-name=\"{server_name}\"] li.episode a[data-episode-id=\"{ep_id}\"]");
            if let Some(link) = select(&doc, &selector).first() {
                let data_id = attr(link, "data-id");
                if !data_id.is_empty() {
                    out.push(VideoHoster {
                        key: format!("{data_id}|{episode_url}|{name}"),
                        name,
                        url: Some(episode_url.clone()),
                        lazy: true,
                        video_count: Some(1),
                        headers: referer_headers(&episode_url),
                        ..VideoHoster::default()
                    });
                }
            }
        }
        Ok(out)
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let mut parts = key.splitn(3, '|');
        let data_id = parts.next().unwrap_or_default();
        let referer = parts.next().unwrap_or(ANIMEWORLD);
        let name = parts.next().unwrap_or("AnimeWorld");
        let api = format!("{ANIMEWORLD}/api/episode/info?id={data_id}&alt=0");
        let body = client(ANIMEWORLD, referer)
            .get(api)
            .xhr()
            .referer(referer)
            .send_text()
            .unwrap_or_else(|_| r#"{"grabber":"https://example.invalid"}"#.to_string());
        let target = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("grabber").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        let mut streams = resolve_embed(&target, name, referer, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        streams_from_hosters(self, request)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        home_sections(self, request, "Popolari", "Aggiornati")
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute(ANIMEWORLD, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|path| absolute(ANIMEWORLD, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        handle_url(ANIMEWORLD, &request, |path| animeworld_details(&path))
    }
}

pub struct AnimeUnitySource;

impl manatan_extension::source::VideoSource for AnimeUnitySource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            let body = fetch(
                ANIMEUNITY,
                &format!("{ANIMEUNITY}/?anime={page}"),
                ANIMEUNITY_LATEST_FIXTURE,
            );
            return Ok(animeunity_latest(&body));
        }
        let body = fetch(
            ANIMEUNITY,
            &format!("{ANIMEUNITY}/top-anime?popular=true&page={page}"),
            ANIMEUNITY_TOP_FIXTURE,
        );
        Ok(animeunity_top(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(path) = path_from_url(ANIMEUNITY, &query) {
            return Ok(Paged {
                entries: vec![animeunity_details(&path)],
                has_next_page: false,
            });
        }
        if query.is_empty() && filters_default(&request) {
            return self.list(request);
        }
        let page = page(&request);
        let archive = client(ANIMEUNITY, ANIMEUNITY)
            .get(format!("{ANIMEUNITY}/archivio"))
            .browser_document()
            .send_text()
            .unwrap_or_else(|_| ANIMEUNITY_ARCHIVE_FIXTURE.to_string());
        let token = html::attr_after(&archive, "meta name=\"csrf-token\"", "content")
            .or_else(|| html::attr_after(&archive, "name=\"csrf-token\"", "content"))
            .unwrap_or_default();
        let payload = json!({
            "title": if query.is_empty() { Value::Bool(false) } else { Value::String(query) },
            "type": filter_value(&request, "type", "").into_json_false(),
            "year": filter_value(&request, "year", "").into_json_false(),
            "order": filter_value(&request, "order", "").into_json_false(),
            "status": filter_value(&request, "status", "").into_json_false(),
            "genres": genre_json(&request),
            "offset": (page - 1) * 30,
            "dubbed": Value::Bool(!filter_value(&request, "dubbed", "").is_empty()),
            "season": filter_value(&request, "season", "").into_json_false(),
        });
        let body = client(ANIMEUNITY, &format!("{ANIMEUNITY}/archivio"))
            .post(format!("{ANIMEUNITY}/archivio/get-animes"))
            .xhr()
            .origin(ANIMEUNITY)
            .referer(format!("{ANIMEUNITY}/archivio"))
            .header("X-CSRF-TOKEN", token)
            .json(payload.to_string())
            .send_text()
            .unwrap_or_else(|_| ANIMEUNITY_SEARCH_FIXTURE.to_string());
        Ok(animeunity_search(&body, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(animeunity_details(
            &request_key(&request, "item").unwrap_or_default(),
        ))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = fetch(ANIMEUNITY, &format!("{ANIMEUNITY}/anime/{key}"), ANIMEUNITY_DETAILS_FIXTURE);
        let episodes = attr_component_json(&body, "video-player", "episodes")
            .and_then(|value| serde_json::from_str::<Value>(&value).ok())
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut out = episodes
            .iter()
            .filter_map(|ep| {
                let id = ep.get("id").and_then(Value::as_i64)?;
                let number = ep.get("number").and_then(Value::as_str).unwrap_or("1");
                Some(VideoEpisode {
                    key: format!("{key}/{id}"),
                    title: Some(format!("Episode {number}")),
                    episode_number: number.split('-').next().and_then(|v| v.parse().ok()),
                    date_uploaded: None,
                    url: Some(format!("{ANIMEUNITY}/anime/{key}/{id}")),
                    language: Some("it".to_string()),
                    ..VideoEpisode::default()
                })
            })
            .collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let page_url = if key.starts_with("http") {
            key
        } else {
            format!("{ANIMEUNITY}/anime/{key}")
        };
        let body = fetch(ANIMEUNITY, &page_url, ANIMEUNITY_EPISODE_FIXTURE);
        let iframe = attr_component_json(&body, "video-player", "embed_url")
            .unwrap_or_else(|| first_url_containing(&body, "vixcloud").unwrap_or_default());
        if iframe.is_empty() {
            return Ok(Vec::new());
        }
        let mut streams = vixcloud_streams(&absolute(ANIMEUNITY, &iframe), ANIMEUNITY, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        home_sections(self, request, "Top Anime", "Ultimi episodi")
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| format!("{ANIMEUNITY}/anime/{key}")))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| format!("{ANIMEUNITY}/anime/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = input.split("/anime/").nth(1).map(|v| v.trim_matches('/').to_string()) {
            return Ok(Some(UrlResolveResult {
                item: Some(animeunity_details(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        search_resolve(input)
    }
}

pub struct AniPlaySource;

impl manatan_extension::source::VideoSource for AniPlaySource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        if listing(&request) == "latest" {
            let body = api_fetch(ANIPLAY, &format!("{ANIPLAY_API}/latest-episodes?page={page}&type=All"), ANIPLAY_LATEST_FIXTURE);
            return Ok(aniplay_latest(&body));
        }
        let body = api_fetch(ANIPLAY, &format!("{ANIPLAY_API}/advancedSearch?sort=7&page={page}&origin=,,,,,,"), ANIPLAY_LIST_FIXTURE);
        Ok(aniplay_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(id) = query.strip_prefix("id:").or_else(|| query.split("/series/").nth(1)) {
            return Ok(Paged { entries: vec![aniplay_details(&format!("/series/{id}"))], has_next_page: false });
        }
        let target = format!(
            "{ANIPLAY_API}/advancedSearch?page={}&origin=,,,,,,&sort={}&_q={}&genres={}&country={}&types={}&studios={}&status={}&subbed={}",
            page(&request),
            filter_value(&request, "sort", "1"),
            url::query_escape(&query),
            filter_value(&request, "genres", ""),
            filter_value(&request, "country", ""),
            filter_value(&request, "types", ""),
            filter_value(&request, "studios", ""),
            filter_value(&request, "status", ""),
            filter_value(&request, "subbed", "")
        );
        Ok(aniplay_listing(&api_fetch(ANIPLAY, &target, ANIPLAY_LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(aniplay_details(&request_key(&request, "item").unwrap_or_default()))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let body = fetch(ANIPLAY, &absolute(ANIPLAY, &key), ANIPLAY_DETAILS_FIXTURE);
        let data = page_data_script(&body);
        let episodes = json_slice_after(&data, ",episodes:", "]},").unwrap_or_else(|| "[]".to_string());
        let value: Value = serde_json::from_str(&fix_js_object(&episodes)).unwrap_or_default();
        let mut out = value.as_array().into_iter().flatten().filter_map(|ep| {
            let id = ep.get("id").and_then(Value::as_i64)?;
            let number = ep.get("number").and_then(Value::as_str).unwrap_or("1");
            Some(VideoEpisode {
                key: format!("/watch/{id}"),
                title: ep.get("title").and_then(Value::as_str).map(str::to_string).or_else(|| Some(format!("Episodio {number}"))),
                episode_number: number.parse().ok(),
                url: Some(format!("{ANIPLAY}/watch/{id}")),
                language: Some("it".to_string()),
                ..VideoEpisode::default()
            })
        }).collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "episode").unwrap_or_default();
        let body = fetch(ANIPLAY, &absolute(ANIPLAY, &key), ANIPLAY_EPISODE_FIXTURE);
        let data = page_data_script(&body);
        let episode = json_object_after(&data, "{episode:", ",views").unwrap_or_else(|| "{}".to_string());
        let value: Value = serde_json::from_str(&fix_js_object(&episode)).unwrap_or_default();
        let link = value.get("streaming_link").or_else(|| value.get("download_link")).and_then(Value::as_str).unwrap_or_default();
        let mut streams = if link.contains(".m3u8") { hls_streams(link, ANIPLAY, "AniPlay", &request) } else { vec![direct_stream(link, "AniPlay", "Default", ANIPLAY, &request)] };
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        home_sections(self, request, "Popolari", "Ultimi episodi")
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| absolute(ANIPLAY, &key)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode").map(|key| absolute(ANIPLAY, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        handle_url(ANIPLAY, &request, |path| aniplay_details(&path))
    }
}

pub struct ToonItaliaSource;

impl manatan_extension::source::VideoSource for ToonItaliaSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = format!("{TOONITALIA}/page/{}", page(&request));
        Ok(toon_listing(&fetch(TOONITALIA, &target, TOON_LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        let page = page(&request);
        let target = if !query.is_empty() {
            format!("{TOONITALIA}/page/{page}/?s={}", url::query_escape(&query))
        } else {
            let index = filter_value(&request, "index", "lista-anime-e-cartoni");
            format!("{TOONITALIA}/{index}/?lcp_page0={page}#lcp_instance_0")
        };
        let body = fetch(TOONITALIA, &target, TOON_LIST_FIXTURE);
        if query.is_empty() {
            Ok(toon_index_listing(&body))
        } else {
            Ok(toon_listing(&body))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        Ok(toon_details(&request_key(&request, "item").unwrap_or_default()))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let path = request_key(&request, "item").unwrap_or_default();
        let page_url = absolute(TOONITALIA, &path);
        let body = fetch(TOONITALIA, &page_url, TOON_DETAILS_FIXTURE);
        if page_url.contains("/film-anime/") {
            let doc = Html::parse_document(&body);
            return Ok(vec![VideoEpisode {
                key: format!("{page_url}#0"),
                title: select(&doc, "h1.entry-title").first().map(text),
                episode_number: Some(1.0),
                url: Some(format!("{page_url}#0")),
                language: Some("it".to_string()),
                ..VideoEpisode::default()
            }]);
        }
        let doc = Html::parse_document(&body);
        let rows = select(&doc, "article > div.entry-content table tr:has(a)");
        let mut out = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let row_text = text(row);
                let title = episode_season_title(&row_text);
                VideoEpisode {
                    key: format!("{page_url}#{index}"),
                    title: Some(title.clone()),
                    episode_number: toon_episode_number(&title),
                    url: Some(format!("{page_url}#{index}")),
                    language: Some("it".to_string()),
                    ..VideoEpisode::default()
                }
            })
            .collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    fn hosters(&self, request: Value) -> ExtensionResult<Vec<VideoHoster>> {
        let episode = request_key(&request, "episode").unwrap_or_default();
        let (page_url, index) = split_fragment(&episode);
        let body = fetch(TOONITALIA, &page_url, TOON_DETAILS_FIXTURE);
        let doc = Html::parse_document(&body);
        let rows = select(&doc, "article > div.entry-content table tr:has(a)");
        let Some(row) = rows.get(index) else {
            return Ok(Vec::new());
        };
        Ok(select_fragment(row, "a")
            .into_iter()
            .map(|a| {
                let href = attr(&a, "href");
                let name = host_name(&href);
                VideoHoster {
                    key: href.clone(),
                    name,
                    url: Some(href),
                    lazy: true,
                    video_count: Some(1),
                    headers: referer_headers(&page_url),
                    ..VideoHoster::default()
                }
            })
            .collect())
    }

    fn resolve_hoster(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let key = request_key(&request, "hoster").unwrap_or_default();
        let target = if key.contains("uprot.net") { bypass_uprot(&key) } else { key };
        let mut streams = resolve_embed(&target, &host_name(&target), TOONITALIA, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        streams_from_hosters(self, request)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popolari".to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|path| absolute(TOONITALIA, &path)))
    }

    fn episode_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "episode"))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        handle_url(TOONITALIA, &request, |path| toon_details(&path))
    }
}

pub struct VvvvidSource;

impl manatan_extension::source::VideoSource for VvvvidSource {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let conn = vvvvid_login()?;
        let channel = vvvvid_channel(&conn.0, if listing(&request) == "latest" { "Nuove uscite" } else { "Popolari" }, "anime");
        let suffix = if page(&request) == 1 { "/last" } else { "" };
        let target = format!("{VVVVID}/vvvvid/ondemand/anime/channel/{channel}{suffix}?conn_id={}", conn.0);
        Ok(vvvvid_listing(&vvvvid_get(&target, &conn), page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if !query.is_empty() {
            return Err(ExtensionError {
                message: "search unavailable for VVVVID".to_string(),
            });
        }
        let conn = vvvvid_login()?;
        let page_kind = filter_value(&request, "page", "anime");
        let channel = filter_value(&request, "channel", "");
        let target = if channel.is_empty() {
            let id = vvvvid_channel(&conn.0, "Popolari", &page_kind);
            format!("{VVVVID}/vvvvid/ondemand/{page_kind}/channel/{id}/last?conn_id={}", conn.0)
        } else {
            format!("{VVVVID}/vvvvid/ondemand/{page_kind}/channel/{channel}/last?conn_id={}", conn.0)
        };
        Ok(vvvvid_listing(&vvvvid_get(&target, &conn), page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = request_key(&request, "item").unwrap_or_default();
        let conn = vvvvid_login()?;
        let body = vvvvid_get(&format!("{VVVVID}/vvvvid/ondemand/{key}/info/?conn_id={}", conn.0), &conn);
        Ok(vvvvid_details(&body, &key))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let key = request_key(&request, "item").unwrap_or_default();
        let conn = vvvvid_login()?;
        let body = vvvvid_get(&format!("{VVVVID}/vvvvid/ondemand/{key}/seasons/?conn_id={}", conn.0), &conn);
        Ok(vvvvid_episodes(&body, &request))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode = request_key(&request, "episode").unwrap_or_default();
        let data: Value = serde_json::from_str(&episode).unwrap_or_default();
        let show_id = data.get("show_id").and_then(Value::as_i64).unwrap_or_default();
        let season_id = data.get("season_id").and_then(Value::as_i64).unwrap_or_default();
        let video_id = data.get("video_id").and_then(Value::as_i64).unwrap_or_default();
        let conn = vvvvid_login()?;
        let body = vvvvid_get(&format!("{VVVVID}/vvvvid/ondemand/{show_id}/season/{season_id}?video_id={video_id}&conn_id={}", conn.0), &conn);
        let mut streams = vvvvid_streams(&body, video_id, &request);
        sort_streams(&mut streams, &request);
        Ok(streams)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        home_sections(self, request, "Popolari", "Nuove uscite")
    }

    fn item_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(request_key(&request, "item").map(|key| format!("{VVVVID}/show/{key}")))
    }
}

fn client(base: &str, referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_origin(base)
        .with_referer(referer)
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch(base: &str, target: &str, fixture: &str) -> String {
    client(base, base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_fetch(base: &str, target: &str, fixture: &str) -> String {
    client(base, base)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn select<'a>(doc: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|sel| doc.select(&sel).collect())
        .unwrap_or_default()
}

fn select_fragment<'a>(el: &'a ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    Selector::parse(selector)
        .ok()
        .map(|sel| el.select(&sel).collect())
        .unwrap_or_default()
}

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
}

fn text(el: &ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_attr(el: &ElementRef<'_>, selector: &str, attr_name: &str) -> Option<String> {
    select_fragment(el, selector).first().map(|el| attr(el, attr_name)).filter(|v| !v.is_empty())
}

fn first_text(el: &ElementRef<'_>, selector: &str) -> Option<String> {
    select_fragment(el, selector).first().map(text).filter(|v| !v.is_empty())
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .or_else(|| request.get("pageNumber"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
}

fn query(request: &Value) -> String {
    request.get("query").and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

fn listing(request: &Value) -> &str {
    request
        .get("listing")
        .or_else(|| request.get("listingId"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter_value(request: &Value, id: &str, default: &str) -> String {
    request
        .pointer(&format!("/filters/{id}"))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn pref_value(request: &Value, id: &str, default: &str) -> String {
    request
        .pointer(&format!("/preferences/{id}"))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
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

fn absolute(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), href.trim_start_matches('/'))
    }
}

fn path_key(base: &str, href: &str) -> String {
    let url = absolute(base, href);
    url.trim_start_matches(base).trim_start_matches('/').split('#').next().unwrap_or_default().to_string().insert_prefix("/")
}

fn path_from_url(base: &str, input: &str) -> Option<String> {
    input.strip_prefix(base).map(|v| v.to_string()).filter(|v| !v.is_empty())
}

trait InsertPrefix {
    fn insert_prefix(self, prefix: &str) -> String;
}

impl InsertPrefix for String {
    fn insert_prefix(self, prefix: &str) -> String {
        if self.starts_with(prefix) { self } else { format!("{prefix}{self}") }
    }
}

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn format_title(title: &str) -> String {
    title
        .replace("(ITA) ITA", "Dub ITA")
        .replace("(ITA)", "Dub ITA")
        .replace("Sub ITA", "")
        .trim()
        .to_string()
}

fn has_next_page(body: &str) -> bool {
    body.contains("page-item active") && !body.contains("page-item active disabled")
        || body.contains("rel=\"next\"")
        || body.contains("go-next-page")
        || body.contains("lcp_nextlink")
}

fn first_number(input: &str) -> Option<f32> {
    Regex::new(r"(\d+(?:\.\d+)?)").ok()?.captures(input)?.get(1)?.as_str().parse().ok()
}

fn filter_params(request: &Value) -> String {
    let mut out = Vec::new();
    for (field, param) in [
        ("genre", "categories%5B0%5D"),
        ("year", "years%5B0%5D"),
        ("state", "states%5B0%5D"),
        ("language", "language%5B0%5D"),
    ] {
        let value = filter_value(request, field, "");
        if !value.is_empty() {
            out.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    out.join("&")
}

fn saturn_cards<C: SaturnConfig>(body: &str) -> Vec<CatalogItem> {
    let doc = Html::parse_document(body);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for selector in [
        "div.sebox",
        "div.card.mb-4.shadow-sm",
        "div.anime-card-newanime.main-anime-card",
        "div.hentai-card-newhentai.main-hentai-card",
        "li.list-group-item",
        "div.item-archivio",
        "div.col-md-2.float-left.hentai-img-box-col.hentai-padding-top",
    ] {
        for el in select(&doc, selector) {
            let href = first_attr(&el, "a[href]", "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            let key = path_key(C::BASE_URL, &href);
            if !seen.insert(key.clone()) {
                continue;
            }
            let title = first_attr(&el, "a[title]", "title")
                .or_else(|| first_attr(&el, "img[title]", "title"))
                .or_else(|| first_attr(&el, "img[alt]", "alt"))
                .or_else(|| first_text(&el, "h2 a, h3 a, a.badge"))
                .unwrap_or_else(|| title_from_key(&key));
            let cover = first_attr(&el, "img", "src");
            out.push(CatalogItem {
                key: key.clone(),
                title: format_title(&title),
                cover: cover.map(|src| absolute(C::BASE_URL, &src)),
                url: Some(absolute(C::BASE_URL, &key)),
                language: Some("it".to_string()),
                content_rating: Some(C::CONTENT_RATING.to_string()),
                status: ItemStatus::Unknown,
                ..CatalogItem::default()
            });
        }
    }
    out
}

fn saturn_details<C: SaturnConfig>(path: &str) -> CatalogItem {
    let body = fetch(C::BASE_URL, &absolute(C::BASE_URL, path), SATURN_DETAILS_FIXTURE);
    let doc = Html::parse_document(&body);
    let title = select(&doc, C::TITLE_CLASS)
        .first()
        .map(text)
        .or_else(|| select(&doc, "h1, h2.title").first().map(text))
        .unwrap_or_else(|| title_from_key(path));
    let detail_text = select(&doc, "div.container.shadow.rounded.bg-dark-as-box")
        .first()
        .map(text)
        .unwrap_or_default();
    let status = if detail_text.contains("Finito") {
        ItemStatus::Completed
    } else if detail_text.contains("In corso") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    };
    let tags = select(&doc, "a.badge.badge-dark.generi-as, a.generi-as")
        .into_iter()
        .map(|el| text(&el))
        .collect();
    let desc = select(&doc, "div#full-trama, div#shown-trama, div#trama")
        .into_iter()
        .map(|el| text(&el))
        .max_by_key(|value| value.len());
    CatalogItem {
        key: path_key(C::BASE_URL, path),
        title: format_title(&title),
        cover: select(&doc, "img.img-fluid.cover-anime.rounded, img.img-fluid.w-100.rounded, img")
            .first()
            .map(|el| absolute(C::BASE_URL, &attr(el, "src"))),
        description: desc,
        tags,
        language: Some("it".to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        status,
        initialized: true,
        url: Some(absolute(C::BASE_URL, path)),
        ..CatalogItem::default()
    }
}

fn title_from_key(key: &str) -> String {
    key.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Anime")
        .replace('-', " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_href_containing(body: &str, needle: &str) -> Option<String> {
    Regex::new(r#"<a[^>]+href=["']([^"']+)["']"#)
        .ok()?
        .captures_iter(body)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .find(|href| href.contains(needle))
}

fn first_url_containing(body: &str, needle: &str) -> Option<String> {
    Regex::new(r#"https?://[^"'\s<>]+"#)
        .ok()?
        .find_iter(body)
        .map(|m| m.as_str().to_string())
        .find(|url| url.contains(needle))
}

fn streams_from_player(body: &str, referer: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let src = Regex::new(r#"file:\s*["']([^"']+)["']"#)
        .ok()
        .and_then(|re| re.captures(body).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string())))
        .or_else(|| html::attr_after(body, "<source", "src"))
        .unwrap_or_default();
    if src.is_empty() {
        Vec::new()
    } else if src.contains(".m3u8") {
        hls_streams(&src, referer, name, request)
    } else {
        vec![direct_stream(&src, name, "Qualita predefinita", referer, request)]
    }
}

fn hls_streams(target: &str, referer: &str, name: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(referer, referer)
        .get(target)
        .referer(referer)
        .send_text()
        .unwrap_or_default();
    if !body.contains("#EXT-X-STREAM-INF") {
        return vec![hls_stream(target, name, "HLS", referer, request)];
    }
    body.split("#EXT-X-STREAM-INF:")
        .skip(1)
        .filter_map(|block| {
            let quality = block
                .split("RESOLUTION=")
                .nth(1)
                .and_then(|part| part.split('x').nth(1))
                .and_then(|part| part.split([',', '\n', '\r']).next())
                .map(|height| format!("{height}p"))
                .unwrap_or_else(|| "HLS".to_string());
            let line = block.lines().find(|line| !line.trim().is_empty() && !line.starts_with('#'))?;
            Some(hls_stream(&absolute(target.rsplit_once('/').map(|v| v.0).unwrap_or(target), line.trim()), name, &quality, referer, request))
        })
        .collect()
}

fn direct_stream(url: &str, name: &str, quality: &str, referer: &str, request: &Value) -> VideoStream {
    let mut headers = referer_headers(referer);
    headers.insert("User-Agent".to_string(), pref_value(request, "preferred_user_agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36").to_string());
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some(quality.to_string()),
        format: Some("mp4".to_string()),
        stream_kind: Some(VideoStreamKind::Direct),
        headers,
        initialized: true,
        ..VideoStream::default()
    }
}

fn hls_stream(url: &str, name: &str, quality: &str, referer: &str, request: &Value) -> VideoStream {
    let mut stream = direct_stream(url, name, quality, referer, request);
    stream.format = Some("hls".to_string());
    stream.is_hls = true;
    stream.stream_kind = Some(VideoStreamKind::Hls);
    stream
}

fn external_stream(url: &str, name: &str) -> VideoStream {
    VideoStream {
        url: url.to_string(),
        name: Some(name.to_string()),
        quality: Some("external".to_string()),
        format: Some("external".to_string()),
        stream_kind: Some(VideoStreamKind::External),
        initialized: true,
        ..VideoStream::default()
    }
}

fn resolve_embed(target: &str, name: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    if target.is_empty() {
        return Vec::new();
    }
    if target.contains(".m3u8") {
        return hls_streams(target, referer, name, request);
    }
    if target.contains(".mp4") || target.contains(".webm") {
        return vec![direct_stream(target, name, "Direct", referer, request)];
    }
    if target.contains("vixcloud") {
        return vixcloud_streams(target, referer, request);
    }
    let body = client(referer, referer).get(target).browser_document().referer(referer).send_text().unwrap_or_default();
    let streams = streams_from_player(&body, target, name, request);
    if streams.is_empty() {
        vec![external_stream(target, name)]
    } else {
        streams
    }
}

fn vixcloud_streams(iframe_url: &str, base: &str, request: &Value) -> Vec<VideoStream> {
    let body = client(base, base)
        .get(iframe_url)
        .browser_document()
        .referer(format!("{base}/"))
        .send_text()
        .unwrap_or_default();
    let script = body.split("masterPlaylist").nth(1).unwrap_or(&body);
    let playlist = js_value(script, "url").unwrap_or_default();
    if playlist.is_empty() {
        return vec![external_stream(iframe_url, "VixCloud")];
    }
    let token = js_value(script, "token").unwrap_or_default();
    let expires = js_value(script, "expires").unwrap_or_default();
    let separator = if playlist.contains('?') { '&' } else { '?' };
    let master = format!("{playlist}{separator}h=1&token={token}&expires={expires}");
    vec![hls_stream(&master, "VixCloud", "HLS", iframe_url, request)]
}

fn js_value(script: &str, key: &str) -> Option<String> {
    for needle in [
        format!("{key}: '"),
        format!("'{key}': '"),
        format!("{key}:\""),
        format!("\"{key}\":\""),
    ] {
        if let Some(value) = script
            .split(&needle)
            .nth(1)
            .and_then(|tail| tail.split(['\'', '"']).next())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
    let preferred = pref_value(request, "preferred_quality", "1080");
    let server = pref_value(request, "preferred_server", "");
    streams.sort_by_key(|stream| {
        let quality = stream.quality.as_deref().unwrap_or_default();
        let name = stream.name.as_deref().unwrap_or_default();
        (
            quality.contains(&preferred),
            name.to_lowercase().contains(&server.to_lowercase()),
            quality_number(quality),
        )
    });
    streams.reverse();
}

fn quality_number(value: &str) -> u32 {
    value.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

fn streams_from_hosters<S: manatan_extension::source::VideoSource>(source: &S, request: Value) -> ExtensionResult<Vec<VideoStream>> {
    let mut streams = Vec::new();
    for hoster in source.hosters(request.clone())? {
        let mut resolved = source.resolve_hoster(json!({
            "hoster": { "key": hoster.key, "name": hoster.name },
            "preferences": request.get("preferences").cloned().unwrap_or(Value::Null),
        }))?;
        for stream in &mut resolved {
            stream.hoster = Some(hoster.clone());
        }
        streams.extend(resolved);
    }
    sort_streams(&mut streams, &request);
    Ok(streams)
}

fn home_sections<S: manatan_extension::source::VideoSource>(source: &S, request: Value, popular_title: &str, latest_title: &str) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
    let popular = source.list(with_listing(&request, "popular"))?;
    let latest = source.list(with_listing(&request, "latest"))?;
    Ok(vec![
        HomeSection {
            id: "popular".to_string(),
            title: popular_title.to_string(),
            style: Some(HomeSectionStyle::Featured),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        },
        HomeSection {
            id: "latest".to_string(),
            title: latest_title.to_string(),
            entries: latest.entries,
            has_more: latest.has_next_page,
            ..HomeSection::default()
        },
    ])
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut request = request.clone();
    if let Some(obj) = request.as_object_mut() {
        obj.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn handle_url<F: Fn(String) -> CatalogItem>(base: &str, request: &Value, details: F) -> ExtensionResult<Option<UrlResolveResult>> {
    let Some(input) = request.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    if let Some(path) = path_from_url(base, input) {
        return Ok(Some(UrlResolveResult {
            item: Some(details(path)),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }));
    }
    search_resolve(input)
}

fn search_resolve(input: &str) -> ExtensionResult<Option<UrlResolveResult>> {
    Ok(Some(UrlResolveResult {
        search: Some(SearchRequest {
            query: input.to_string(),
            ..SearchRequest::default()
        }),
        url: Some(input.to_string()),
        ..UrlResolveResult::default()
    }))
}

fn filters_default(request: &Value) -> bool {
    request.get("filters").and_then(Value::as_object).map(|obj| obj.values().all(|v| v.as_str().unwrap_or_default().is_empty())).unwrap_or(true)
}

trait JsonFalse {
    fn into_json_false(self) -> Value;
    fn not_value(self) -> Value;
}

impl JsonFalse for String {
    fn into_json_false(self) -> Value {
        if self.is_empty() { Value::Bool(false) } else { Value::String(self) }
    }

    fn not_value(self) -> Value {
        Value::Bool(!self.is_empty())
    }
}

fn genre_json(request: &Value) -> Value {
    let value = filter_value(request, "genre", "");
    if value.is_empty() {
        Value::Bool(false)
    } else {
        serde_json::from_str(&value).unwrap_or_else(|_| Value::String(value))
    }
}

fn animeworld_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select(&doc, "div.film-list div.item div.inner a.poster")
            .into_iter()
            .filter_map(|a| {
                let href = attr(&a, "href");
                let img = select_fragment(&a, "img").first().cloned();
                Some(CatalogItem {
                    key: path_key(ANIMEWORLD, &href),
                    title: img.as_ref().map(|i| attr(i, "alt")).filter(|v| !v.is_empty()).unwrap_or_else(|| title_from_key(&href)),
                    cover: img.as_ref().map(|i| absolute(ANIMEWORLD, &attr(i, "src"))),
                    url: Some(absolute(ANIMEWORLD, &href)),
                    language: Some("it".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("go-next-page"),
    }
}

fn animeworld_filter_params(request: &Value) -> String {
    let mut out = Vec::new();
    for (field, param) in [
        ("genre", "genre"),
        ("season", "season"),
        ("year", "year"),
        ("type", "type"),
        ("status", "status"),
        ("studio", "studio"),
        ("dub", "dub"),
        ("language", "language"),
        ("sort", "sort"),
    ] {
        let value = filter_value(request, field, if field == "sort" { "0" } else { "" });
        if !value.is_empty() {
            out.push(format!("{param}={}", url::query_escape(&value)));
        }
    }
    out.join("&")
}

fn animeworld_details(path: &str) -> CatalogItem {
    let body = fetch(ANIMEWORLD, &absolute(ANIMEWORLD, path), ANIMEWORLD_DETAILS_FIXTURE);
    let doc = Html::parse_document(&body);
    let dl_text = select(&doc, "div.info dl").first().map(text).unwrap_or_default();
    CatalogItem {
        key: path_key(ANIMEWORLD, path),
        title: select(&doc, "div.c1 h2.title").first().map(text).unwrap_or_else(|| title_from_key(path)),
        cover: select(&doc, "div.thumb img").first().map(|el| absolute(ANIMEWORLD, &attr(el, "src"))),
        tags: select(&doc, "dd a[href*=language], dd a[href*=genre]").into_iter().map(|el| text(&el)).collect(),
        authors: select(&doc, "dd a[href*=studio]").into_iter().map(|el| text(&el)).collect(),
        description: select(&doc, "div.desc").first().map(text),
        status: if dl_text.contains("Finito") { ItemStatus::Completed } else if dl_text.contains("In corso") { ItemStatus::Ongoing } else { ItemStatus::Unknown },
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        url: Some(absolute(ANIMEWORLD, path)),
        ..CatalogItem::default()
    }
}

fn animeunity_top(body: &str) -> Paged<CatalogItem> {
    let raw = body
        .split("top-anime animes=\"")
        .nth(1)
        .and_then(|tail| tail.split("\"></top-anime>").next())
        .map(html::html_unescape)
        .unwrap_or_else(|| ANIMEUNITY_TOP_JSON.to_string());
    let value: Value = serde_json::from_str(&raw).unwrap_or_default();
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(animeunity_item)
        .collect();
    Paged {
        entries,
        has_next_page: value.get("current_page").and_then(Value::as_i64).unwrap_or(1)
            < value.get("last_page").and_then(Value::as_i64).unwrap_or(1),
    }
}

fn animeunity_latest(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select(&doc, "div.home-wrapper-body div.latest-anime-container")
            .into_iter()
            .filter_map(|el| {
                let href = first_attr(&el, "a[href]", "href")?;
                Some(CatalogItem {
                    key: href.split("/anime/").nth(1).unwrap_or(&href).to_string(),
                    title: first_text(&el, "a strong").unwrap_or_else(|| title_from_key(&href)),
                    cover: first_attr(&el, "img", "src"),
                    url: Some(absolute(ANIMEUNITY, &href)),
                    language: Some("it".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("pagination") && body.contains("active"),
    }
}

fn animeunity_search(body: &str, page: u64) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = value
        .get("records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(animeunity_item)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: value.get("tot").and_then(Value::as_u64).unwrap_or(0) > page * 30,
        entries,
    }
}

fn animeunity_item(value: &Value) -> Option<CatalogItem> {
    let id = value.get("id").and_then(Value::as_i64)?;
    let slug = value.get("slug").and_then(Value::as_str)?;
    let title = value.get("title_eng").or_else(|| value.get("title")).and_then(Value::as_str)?;
    Some(CatalogItem {
        key: format!("{id}-{slug}"),
        title: title.to_string(),
        cover: value.get("imageurl").and_then(Value::as_str).map(str::to_string),
        url: Some(format!("{ANIMEUNITY}/anime/{id}-{slug}")),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn animeunity_details(key: &str) -> CatalogItem {
    let body = fetch(ANIMEUNITY, &format!("{ANIMEUNITY}/anime/{key}"), ANIMEUNITY_DETAILS_FIXTURE);
    let info = attr_component_json(&body, "video-player", "anime")
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: info.get("title_eng").and_then(Value::as_str).unwrap_or_else(|| key).to_string(),
        cover: info.get("imageurl").and_then(Value::as_str).map(str::to_string),
        description: info.get("plot").and_then(Value::as_str).map(str::to_string),
        tags: info.get("genres").and_then(Value::as_array).into_iter().flatten().filter_map(|g| g.get("name").and_then(Value::as_str)).map(str::to_string).collect(),
        authors: info.get("studio").and_then(Value::as_str).map(|s| vec![s.to_string()]).unwrap_or_default(),
        rating: info.get("score").and_then(Value::as_str).and_then(|s| s.parse::<f32>().ok()).map(|s| s / 2.0),
        status: match info.get("status").and_then(Value::as_str).unwrap_or_default() {
            "Terminato" => ItemStatus::Completed,
            "In Corso" => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        url: Some(format!("{ANIMEUNITY}/anime/{key}")),
        ..CatalogItem::default()
    }
}

fn attr_component_json(body: &str, tag: &str, attr_name: &str) -> Option<String> {
    let start = body.find(tag)?;
    html::attr(&body[start..], attr_name).map(|raw| html::html_unescape(&raw))
}

fn aniplay_listing(body: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = value.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(aniplay_item).collect();
    Paged {
        entries,
        has_next_page: value.pointer("/pagination/page").and_then(Value::as_u64).unwrap_or(1)
            < value.pointer("/pagination/pageCount").and_then(Value::as_u64).unwrap_or(1),
    }
}

fn aniplay_latest(body: &str) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = value.as_array().into_iter().flatten().filter_map(|item| item.get("serie").and_then(Value::as_array).and_then(|arr| arr.first()).and_then(aniplay_item)).collect::<Vec<_>>();
    Paged { has_next_page: entries.len() == 20, entries }
}

fn aniplay_item(value: &Value) -> Option<CatalogItem> {
    let id = value.get("id").and_then(Value::as_i64)?;
    let title = value.get("title").or_else(|| value.get("name")).and_then(Value::as_str)?;
    Some(CatalogItem {
        key: format!("/series/{id}"),
        title: title.to_string(),
        cover: value.get("cover").or_else(|| value.get("main_image")).and_then(Value::as_str).map(str::to_string),
        url: Some(format!("{ANIPLAY}/series/{id}")),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn aniplay_details(path: &str) -> CatalogItem {
    let path = if path.starts_with("/series/") { path.to_string() } else { format!("/series/{}", path.trim_matches('/')) };
    let body = fetch(ANIPLAY, &absolute(ANIPLAY, &path), ANIPLAY_DETAILS_FIXTURE);
    let data = page_data_script(&body);
    let json = json_object_after(&data, "{serie:", ",tags").unwrap_or_else(|| "{}".to_string());
    let value: Value = serde_json::from_str(&fix_js_object(&json)).unwrap_or_default();
    CatalogItem {
        key: path.clone(),
        title: value.get("title").and_then(Value::as_str).unwrap_or_else(|| path.as_str()).to_string(),
        cover: value.get("cover").or_else(|| value.get("main_image")).and_then(Value::as_str).map(str::to_string),
        description: value.get("description").and_then(Value::as_str).map(str::to_string),
        tags: value.get("genres").and_then(Value::as_array).into_iter().flatten().filter_map(|g| g.get("name").and_then(Value::as_str)).map(str::to_string).collect(),
        authors: value.get("studios").and_then(Value::as_array).into_iter().flatten().filter_map(|g| g.get("name").and_then(Value::as_str)).map(str::to_string).collect(),
        status: match value.get("status").and_then(Value::as_str).unwrap_or_default() {
            "Completato" => ItemStatus::Completed,
            "In corso" => ItemStatus::Ongoing,
            "Sospeso" => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        url: Some(absolute(ANIPLAY, &path)),
        ..CatalogItem::default()
    }
}

fn page_data_script(body: &str) -> String {
    let doc = Html::parse_document(body);
    select(&doc, "script")
        .into_iter()
        .map(|el| el.inner_html())
        .find(|script| script.contains("const data = "))
        .unwrap_or_default()
}

fn json_object_after(script: &str, start: &str, end: &str) -> Option<String> {
    Some(format!("{}{}", script.split(start).nth(1)?.split(end).next()?, "}"))
}

fn json_slice_after(script: &str, start: &str, end: &str) -> Option<String> {
    Some(format!("{}{}", script.split(start).nth(1)?.split(end).next()?, "]"))
}

fn fix_js_object(input: &str) -> String {
    Regex::new(r#"([a-zA-Z_][a-zA-Z0-9_]*):\s*"#)
        .ok()
        .map(|re| re.replace_all(input, "\"$1\":").to_string())
        .unwrap_or_else(|| input.to_string())
}

fn toon_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select(&doc, "#primary > main#main > article")
            .into_iter()
            .filter_map(|article| {
                let href = first_attr(&article, "h2 > a", "href")?;
                Some(CatalogItem {
                    key: path_key(TOONITALIA, &href),
                    title: first_text(&article, "h2 > a").unwrap_or_else(|| title_from_key(&href)),
                    cover: first_attr(&article, "img", "src"),
                    url: Some(absolute(TOONITALIA, &href)),
                    language: Some("it".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("pagination") && body.contains("next"),
    }
}

fn toon_index_listing(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    Paged {
        entries: select(&doc, "div.entry-content > ul.lcp_catlist > li")
            .into_iter()
            .filter_map(|li| {
                let href = first_attr(&li, "a", "href")?;
                Some(CatalogItem {
                    key: path_key(TOONITALIA, &href),
                    title: first_text(&li, "a").unwrap_or_else(|| title_from_key(&href)),
                    url: Some(absolute(TOONITALIA, &href)),
                    language: Some("it".to_string()),
                    content_rating: Some("safe".to_string()),
                    status: ItemStatus::Unknown,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("lcp_nextlink"),
    }
}

fn toon_details(path: &str) -> CatalogItem {
    let body = fetch(TOONITALIA, &absolute(TOONITALIA, path), TOON_DETAILS_FIXTURE);
    let doc = Html::parse_document(&body);
    let content = select(&doc, "article > div.entry-content").first().map(text).unwrap_or_default();
    CatalogItem {
        key: path_key(TOONITALIA, path),
        title: select(&doc, "h1.entry-title").first().map(text).unwrap_or_else(|| title_from_key(path)),
        cover: select(&doc, "header.entry-header img").first().map(|el| absolute(TOONITALIA, &attr(el, "src"))),
        description: content.split("Trama:").nth(1).map(|v| v.trim().to_string()).or(Some(content.clone())),
        tags: content.split("Genere:").nth(1).and_then(|v| v.lines().next()).map(|line| line.split(',').map(|s| s.trim().to_string()).collect()).unwrap_or_default(),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        url: Some(absolute(TOONITALIA, path)),
        ..CatalogItem::default()
    }
}

fn episode_season_title(input: &str) -> String {
    Regex::new(r"\s(\d+)x(\d+)\s?")
        .ok()
        .and_then(|re| re.captures(input))
        .map(|cap| format!("Stagione {} - Episodi {}", &cap[1], &cap[2]))
        .unwrap_or_else(|| input.to_string())
}

fn toon_episode_number(title: &str) -> Option<f32> {
    let nums = Regex::new(r"(\d+)").ok()?.find_iter(title).map(|m| m.as_str().to_string()).collect::<Vec<_>>();
    if nums.len() >= 2 {
        format!("{}.{}", nums[0], nums[1].pad_left(3)).parse().ok()
    } else {
        first_number(title)
    }
}

trait PadLeft {
    fn pad_left(&self, width: usize) -> String;
}

impl PadLeft for String {
    fn pad_left(&self, width: usize) -> String {
        format!("{:0>width$}", self, width = width)
    }
}

fn split_fragment(input: &str) -> (String, usize) {
    let mut parts = input.split('#');
    let url = parts.next().unwrap_or_default().to_string();
    let index = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (url, index)
}

fn host_name(url: &str) -> String {
    url.split("://").nth(1).and_then(|v| v.split('/').next()).unwrap_or("External").replace("www.", "")
}

fn bypass_uprot(url: &str) -> String {
    let body = fetch(TOONITALIA, url, "");
    Regex::new(r#"<a[^>]+href=["']([^"']+)["'][^>]*>[^<]*Continue"#)
        .ok()
        .and_then(|re| re.captures_iter(&body).filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string())).find(|link| link.contains("maxstream") || link.contains("streamtape") || link.contains("voe") || link.contains("uprot")))
        .unwrap_or_else(|| url.to_string())
}

fn vvvvid_login() -> ExtensionResult<(String, String)> {
    let seed = "1234567890123456";
    let payload = json!({
        "action": "login",
        "email": "",
        "password": "",
        "facebookParams": "",
        "isIframe": false,
        "mobile": false,
        "hls": true,
        "dash": true,
        "flash": false,
        "webm": true,
        "wv+mp4": true,
        "wv+webm": true,
        "pr+mp4": false,
        "pr+webm": false,
        "fp+mp4": false,
        "device_id_seed": seed
    });
    let body = client(VVVVID, &format!("{VVVVID}/channel/0/you"))
        .post(format!("{VVVVID}/user/login"))
        .xhr()
        .origin(VVVVID)
        .referer(format!("{VVVVID}/channel/0/you"))
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| VVVVID_LOGIN_FIXTURE.to_string());
    let value: Value = serde_json::from_str(&body).unwrap_or_default();
    let conn = value.pointer("/data/conn_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let session = value.pointer("/data/sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
    if conn.is_empty() {
        Err(ExtensionError {
            message: "VVVVID login did not return a connection id".to_string(),
        })
    } else {
        Ok((conn, session))
    }
}

fn vvvvid_get(target: &str, conn: &(String, String)) -> String {
    client(VVVVID, VVVVID)
        .get(target)
        .xhr()
        .header("Cookie", format!("JSESSIONID={}", conn.1))
        .referer(format!("{VVVVID}/"))
        .send_text()
        .unwrap_or_else(|_| VVVVID_LIST_FIXTURE.to_string())
}

fn vvvvid_channel(conn_id: &str, wanted: &str, page_kind: &str) -> String {
    let body = client(VVVVID, VVVVID)
        .get(format!("{VVVVID}/vvvvid/ondemand/{page_kind}/channels?conn_id={conn_id}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| VVVVID_CHANNELS_FIXTURE.to_string());
    let value: Value = serde_json::from_str(&body).unwrap_or_default();
    value.get("data").and_then(Value::as_array).into_iter().flatten()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(wanted))
        .and_then(|item| item.get("id").and_then(Value::as_i64))
        .unwrap_or(0)
        .to_string()
}

fn vvvvid_listing(body: &str, _page: u64) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let entries = value.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(|item| {
        let show_id = item.get("show_id").and_then(Value::as_i64)?;
        Some(CatalogItem {
            key: show_id.to_string(),
            title: item.get("title").and_then(Value::as_str).unwrap_or("VVVVID").to_string(),
            cover: item.get("thumbnail").and_then(Value::as_str).map(str::to_string),
            url: Some(format!("{VVVVID}/show/{show_id}")),
            language: Some("it".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { has_next_page: entries.len() == 15, entries }
}

fn vvvvid_details(body: &str, key: &str) -> CatalogItem {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let data = value.get("data").unwrap_or(&value);
    CatalogItem {
        key: key.to_string(),
        title: data.get("title").and_then(Value::as_str).unwrap_or(key).to_string(),
        cover: data.get("thumbnail").and_then(Value::as_str).map(str::to_string),
        description: data.get("description").and_then(Value::as_str).map(|desc| {
            let mut out = desc.to_string();
            if let Some(year) = data.get("date_published").and_then(Value::as_str) {
                out.push_str(&format!("\n\nAnno pubblicato: {year}"));
            }
            if let Some(info) = data.get("additional_info").and_then(Value::as_str) {
                out.push('\n');
                out.push_str(&info.replace(" | ", "\n"));
            }
            out
        }),
        tags: data.get("show_genres").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        url: Some(format!("{VVVVID}/show/{key}")),
        ..CatalogItem::default()
    }
}

fn vvvvid_episodes(body: &str, request: &Value) -> Vec<VideoEpisode> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let sub_pref = pref_value(request, "preferred_sub", "none");
    let mut out = Vec::new();
    let mut counter = 1.0;
    for season in value.get("data").and_then(Value::as_array).into_iter().flatten() {
        let season_name = season.get("name").and_then(Value::as_str).unwrap_or_default().to_lowercase();
        let prefix = if season_name.contains("in italiano") {
            if sub_pref == "sub" { continue; }
            "(Dub) Episodi "
        } else if season_name.contains("in giapponese") {
            if sub_pref == "dub" { continue; }
            "(Sub) Episodi "
        } else {
            season.get("name").and_then(Value::as_str).unwrap_or("")
        };
        for ep in season.get("episodes").and_then(Value::as_array).into_iter().flatten() {
            let show_id = season.get("show_id").and_then(Value::as_i64).unwrap_or_default();
            let season_id = ep.get("season_id").and_then(Value::as_i64).unwrap_or_default();
            let video_id = ep.get("video_id").and_then(Value::as_i64).unwrap_or_default();
            let number = ep.get("number").and_then(Value::as_str).unwrap_or("");
            let title = ep.get("title").and_then(Value::as_str).unwrap_or("");
            out.push(VideoEpisode {
                key: json!({"show_id": show_id, "season_id": season_id, "video_id": video_id}).to_string(),
                title: Some(format!("{prefix}{number} {title}").trim().to_string()),
                episode_number: Some(counter),
                language: Some("it".to_string()),
                ..VideoEpisode::default()
            });
            counter += 1.0;
        }
    }
    out.reverse();
    out
}

fn vvvvid_streams(body: &str, video_id: i64, request: &Value) -> Vec<VideoStream> {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let Some(video) = value.get("data").and_then(Value::as_array).into_iter().flatten().find(|item| item.get("video_id").and_then(Value::as_i64) == Some(video_id)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(url) = video.get("embed_info").and_then(Value::as_str).map(real_vvvvid_url) {
        out.push(vvvvid_dash_stream(&url, "HD", request));
    }
    if let Some(url) = video.get("embed_info_sd").and_then(Value::as_str).map(real_vvvvid_url) {
        out.push(vvvvid_dash_stream(&url, "SD", request));
    }
    out
}

fn vvvvid_dash_stream(url: &str, quality: &str, request: &Value) -> VideoStream {
    let body = client(VVVVID, VVVVID).get(url).send_text().unwrap_or_default();
    let base = url.rsplit_once('/').map(|v| v.0).unwrap_or(url);
    let video = body.split("mimeType=\"video").nth(1).and_then(|v| v.split("</BaseURL>").next()).and_then(|v| v.split("<BaseURL>").nth(1)).unwrap_or_default();
    let audio = body.split("mimeType=\"audio").nth(1).and_then(|v| v.split("</BaseURL>").next()).and_then(|v| v.split("<BaseURL>").nth(1)).unwrap_or_default();
    let mut stream = direct_stream(&format!("{base}/{video}"), "VVVVID", quality, VVVVID, request);
    stream.format = Some("dash".to_string());
    stream.is_dash = true;
    stream.stream_kind = Some(VideoStreamKind::Dash);
    if !audio.is_empty() {
        stream.audio_tracks.push(AudioTrack {
            url: Some(format!("{base}/{audio}")),
            label: Some("Audio".to_string()),
            ..AudioTrack::default()
        });
    }
    stream
}

fn real_vvvvid_url(input: &str) -> String {
    let alphabet = "MNOPIJKL89+/4567UVWXQRSTEFGHABCDcdefYZabstuvopqr0123wxyzklmnghij";
    let mut codes = input.chars().filter_map(|ch| alphabet.find(ch).map(|i| i as i32)).collect::<Vec<_>>();
    if codes.is_empty() {
        return input.to_string();
    }
    let len = codes.len();
    for e in (0..=(len * 2 - 1)).rev() {
        let a = codes[e % len] ^ codes[(e + 1) % len];
        codes[e % len] = a;
    }
    let decoded = vvvvid_decode(&codes);
    decoded.into_iter().filter_map(|n| char::from_u32(n as u32)).collect()
}

fn vvvvid_decode(values: &[i32]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < values.len() {
        let n = values[i] << 2;
        i += 1;
        let Some(second) = values.get(i).copied() else {
            out.push(n);
            break;
        };
        let n = n + (second >> 4);
        i += 1;
        out.push(n);
        if let Some(third) = values.get(i).copied() {
            let k = ((second << 4) & 255) + (third >> 2);
            out.push(k);
            i += 1;
            if let Some(fourth) = values.get(i).copied() {
                out.push(((third << 6) & 255) + fourth);
                i += 1;
            }
        }
    }
    out
}

const ANIMEWORLD: &str = "https://www.animeworld.ac";
const ANIMEUNITY: &str = "https://www.animeunity.so";
const ANIPLAY: &str = "https://aniplay.co";
const ANIPLAY_API: &str = "https://api.aniplay.co/api/series";
const TOONITALIA: &str = "https://toonitalia.green";
const VVVVID: &str = "https://www.vvvvid.it";

const SATURN_LIST_FIXTURE: &str = r#"<div class="card mb-4 shadow-sm"><a href="/anime/sample" title="Sample"><img src="/cover.jpg"></a></div>"#;
const SATURN_DETAILS_FIXTURE: &str = r#"<h1 class="anime-title-as"><b>Sample</b></h1><img class="img-fluid cover-anime rounded" src="/cover.jpg"><div id="trama">Sample</div><div class="btn-group episodes-button episodi-link-button"><a class="btn btn-dark mb-1 bottone-ep" href="/ep/sample-1">Episodio 1</a></div>"#;
const SATURN_EPISODE_FIXTURE: &str = r#"<a href="/watch/sample">watch</a>"#;
const SATURN_WATCH_FIXTURE: &str = r#"<script>jwplayer("player").setup({file: "https://example.invalid/playlist.m3u8"});</script>"#;
const ANIMEWORLD_LIST_FIXTURE: &str = r#"<div class="film-list"><div class="item"><div class="inner"><a class="poster" href="/play/sample"><img src="/cover.jpg" alt="Sample"></a></div></div></div>"#;
const ANIMEWORLD_DETAILS_FIXTURE: &str = r#"<div class="thumb"><img src="/cover.jpg"></div><div class="c1"><h2 class="title">Sample</h2></div><div class="desc">Sample</div><div class="server active"><ul class="episodes"><li class="episode"><a href="/watch/sample-1">1</a></li></ul></div>"#;
const ANIMEWORLD_EPISODE_FIXTURE: &str = r#"<div id="player" data-episode-id="1"></div><div class="servers"><div class="widget-title"><span class="server-tab" data-name="aw">AnimeWorld Server</span></div></div><div class="server" data-name="aw"><li class="episode"><a data-episode-id="1" data-id="1"></a></li></div>"#;
const ANIMEUNITY_TOP_FIXTURE: &str = r#"<top-anime animes="{&quot;current_page&quot;:1,&quot;last_page&quot;:1,&quot;data&quot;:[{&quot;id&quot;:1,&quot;slug&quot;:&quot;sample&quot;,&quot;title_eng&quot;:&quot;Sample&quot;,&quot;imageurl&quot;:&quot;/cover.jpg&quot;}]}"></top-anime>"#;
const ANIMEUNITY_TOP_JSON: &str = r#"{"current_page":1,"last_page":1,"data":[{"id":1,"slug":"sample","title_eng":"Sample","imageurl":"/cover.jpg"}]}"#;
const ANIMEUNITY_LATEST_FIXTURE: &str = r#"<div class="home-wrapper-body"><div class="row"><div class="latest-anime-container"><a href="/anime/1-sample"><strong>Sample</strong></a><img src="/cover.jpg"></div></div></div>"#;
const ANIMEUNITY_ARCHIVE_FIXTURE: &str = r#"<meta name="csrf-token" content="sample">"#;
const ANIMEUNITY_SEARCH_FIXTURE: &str = r#"{"records":[{"id":1,"slug":"sample","title_eng":"Sample","imageurl":"/cover.jpg"}],"tot":1}"#;
const ANIMEUNITY_DETAILS_FIXTURE: &str = r#"<video-player anime="{&quot;title_eng&quot;:&quot;Sample&quot;,&quot;imageurl&quot;:&quot;/cover.jpg&quot;,&quot;plot&quot;:&quot;Sample&quot;,&quot;status&quot;:&quot;In Corso&quot;,&quot;genres&quot;:[]}" episodes="[{&quot;id&quot;:1,&quot;number&quot;:&quot;1&quot;,&quot;created_at&quot;:&quot;2024-01-01 00:00:00&quot;}]" embed_url="https://vixcloud.co/embed/sample"></video-player>"#;
const ANIMEUNITY_EPISODE_FIXTURE: &str = ANIMEUNITY_DETAILS_FIXTURE;
const ANIPLAY_LIST_FIXTURE: &str = r#"{"data":[{"id":1,"title":"Sample","cover":"/cover.jpg"}],"pagination":{"page":1,"pageCount":1}}"#;
const ANIPLAY_LATEST_FIXTURE: &str = r#"[{"serie":[{"id":1,"title":"Sample","cover":"/cover.jpg"}]}]"#;
const ANIPLAY_DETAILS_FIXTURE: &str = r#"<script>const data = {serie:{id:1,title:"Sample",description:"Sample",genres:[],studios:[],cover:"/cover.jpg",status:"In corso"},tags:[],episodes:[{id:1,number:"1",title:"Episode 1"}]},foo=1</script>"#;
const ANIPLAY_EPISODE_FIXTURE: &str = r#"<script>const data = {episode:{id:1,streaming_link:"https://example.invalid/playlist.m3u8"},views:1}</script>"#;
const TOON_LIST_FIXTURE: &str = r#"<main id="main"><article><h2><a href="/sample/">Sample</a></h2><img src="/cover.jpg"></article></main>"#;
const TOON_DETAILS_FIXTURE: &str = r#"<article><header class="entry-header"><h1 class="entry-title">Sample</h1><img src="/cover.jpg"></header><div class="entry-content">Genere: Anime Trama: Sample<table><tr><td>Sample 1x01</td><td><a href="https://streamz.cc/sample">StreamZ</a></td></tr></table></div></article>"#;
const VVVVID_LOGIN_FIXTURE: &str = r#"{"data":{"conn_id":"fixture-conn","sessionId":"fixture-session"}}"#;
const VVVVID_LIST_FIXTURE: &str = r#"{"data":[{"show_id":1,"title":"Sample","thumbnail":"/cover.jpg"}]}"#;
const VVVVID_CHANNELS_FIXTURE: &str = r#"{"data":[{"id":1,"name":"Popolari"},{"id":2,"name":"Nuove uscite"}]}"#;
