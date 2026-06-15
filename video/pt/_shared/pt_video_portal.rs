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

pub trait PortalConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "safe";
    const KIND: PortalKind;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PortalKind {
    AnimesDigital,
    AnimesGames,
    AnimesOnlineVip,
    AnimeCore,
}

pub struct PortalSource<C>(PhantomData<C>);

impl<C> PortalSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: PortalConfig> VideoSource for PortalSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = match (C::KIND, listing(&request)) {
            (PortalKind::AnimesDigital, "latest") => {
                format!("{}/lancamentos/page/{}", C::BASE_URL, page(&request))
            }
            (PortalKind::AnimesDigital, _) => format!("{}/home", C::BASE_URL),
            (PortalKind::AnimesGames, "latest") => {
                format!("{}/lancamentos/page/{}", C::BASE_URL, page(&request))
            }
            (PortalKind::AnimesGames, _) => C::BASE_URL.to_string(),
            (PortalKind::AnimesOnlineVip, "latest") => {
                format!("{}/page/{}", C::BASE_URL, page(&request))
            }
            (PortalKind::AnimesOnlineVip, _) => format!("{}/top-100", C::BASE_URL),
            (PortalKind::AnimeCore, "latest") => {
                return Ok(search_animecore::<C>("updated", page(&request), ""));
            }
            (PortalKind::AnimeCore, _) => {
                return Ok(search_animecore::<C>("popular", page(&request), ""));
            }
        };
        Ok(parse_page_cards::<C>(&fetch::<C>(
            &target,
            LIST_FIXTURE,
            C::BASE_URL,
        )))
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

        let result = match C::KIND {
            PortalKind::AnimesDigital => search_listanime::<C>(
                "animes-legendados-online",
                "anime",
                "animes",
                page(&request),
                query,
                &request,
            ),
            PortalKind::AnimesGames => search_listanime::<C>(
                "lista-de-animes",
                "anime",
                "anime",
                page(&request),
                query,
                &request,
            ),
            PortalKind::AnimesOnlineVip => {
                let target = format!(
                    "{}/page/{}?s={}",
                    C::BASE_URL,
                    page(&request),
                    url::query_escape(query)
                );
                parse_page_cards::<C>(&fetch::<C>(&target, LIST_FIXTURE, C::BASE_URL))
            }
            PortalKind::AnimeCore => search_animecore::<C>("", page(&request), query),
        };
        Ok(result)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let fallback = match C::KIND {
            PortalKind::AnimeCore => "/anime/sample",
            PortalKind::AnimesGames => "/animes/sample",
            _ => "/anime/a/sample",
        };
        let path = request_key::<C>(&request, "item").unwrap_or_else(|| fallback.to_string());
        Ok(fetch_details::<C>(&path))
    }

    fn episodes(&self, request: Value) -> ExtensionResult<Vec<VideoEpisode>> {
        let fallback = if C::KIND == PortalKind::AnimeCore {
            "/anime/sample"
        } else {
            "/sample"
        };
        let path = request_key::<C>(&request, "item").unwrap_or_else(|| fallback.to_string());
        Ok(fetch_episodes::<C>(&path))
    }

    fn streams(&self, request: Value) -> ExtensionResult<Vec<VideoStream>> {
        let episode =
            request_key::<C>(&request, "episode").unwrap_or_else(|| "/sample".to_string());
        let url = absolute_url::<C>(&episode);
        let body = fetch::<C>(&url, PLAYER_FIXTURE, C::BASE_URL);
        let mut streams = match C::KIND {
            PortalKind::AnimesDigital => streams_digital::<C>(&body, &url, &request),
            PortalKind::AnimesGames => streams_games::<C>(&body, &url, &request),
            PortalKind::AnimesOnlineVip => streams_vip::<C>(&body, &url, &request),
            PortalKind::AnimeCore => streams_animecore::<C>(&body, &url, &request),
        };
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
                title: "Lancamentos".to_string(),
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

fn client<C: PortalConfig>(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_header("Origin", C::BASE_URL)
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch<C: PortalConfig>(target: &str, fixture: &str, referer: &str) -> String {
    client::<C>(referer)
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_listanime<C: PortalConfig>(
    token_path: &str,
    type_url: &str,
    default_type: &str,
    page: u64,
    query: &str,
    request: &Value,
) -> Paged<CatalogItem> {
    let token_body = fetch::<C>(
        &format!("{}/{}", C::BASE_URL, token_path),
        LIST_FIXTURE,
        C::BASE_URL,
    );
    let token = Html::parse_document(&token_body)
        .select(&selector("div.menu_filter_box[data-secury]"))
        .next()
        .map(|el| attr(&el, "data-secury"))
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    let audio = filter(request, "audio").unwrap_or_else(|| "0".to_string());
    let letter = filter(request, "letter").unwrap_or_else(|| "0".to_string());
    let order = filter(request, "order").unwrap_or_else(|| "name".to_string());
    let type_filter = filter(request, "type").unwrap_or_else(|| default_type.to_string());
    let filter_data = format!(
        "type_url={}&filter_audio={}&filter_letter={}&filter_order={}&filter_sort=abc",
        url::query_escape(type_url),
        url::query_escape(&audio),
        url::query_escape(&letter),
        url::query_escape(&order)
    );
    let filters = if C::KIND == PortalKind::AnimesDigital {
        format!(
            r#"{{"filter_data": "{}&type_url={}", "filter_genre_add": [], "filter_genre_del": []}}"#,
            filter_data,
            url::query_escape(&type_filter)
        )
    } else {
        format!(
            r#"{{"filter_data": "{}", "filter_genre_add": [], "filter_genre_del": []}}"#,
            filter_data
        )
    };
    let search_value = if C::KIND == PortalKind::AnimesGames && query.is_empty() {
        "0"
    } else {
        query
    };
    let page_s = page.to_string();
    let fields = vec![
        ("pagina", page_s.as_str()),
        ("type", "lista"),
        ("type_url", type_url),
        ("limit", "30"),
        ("token", token.as_str()),
        ("search", search_value),
        ("filters", filters.as_str()),
    ];
    let body = client::<C>(C::BASE_URL)
        .post(format!("{}/func/listanime", C::BASE_URL))
        .xhr()
        .form(&fields)
        .send_text()
        .unwrap_or_else(|_| LIST_JSON_FIXTURE.to_string());
    let Ok(data) = serde_json::from_str::<ListAnimeResponse>(&body) else {
        return Paged::default();
    };
    let entries = data
        .results
        .iter()
        .flat_map(|html| parse_page_cards::<C>(html).entries)
        .collect();
    Paged {
        entries,
        has_next_page: data.total_page > data.page,
    }
}

fn search_animecore<C: PortalConfig>(orderby: &str, page: u64, query: &str) -> Paged<CatalogItem> {
    let page_s = page.to_string();
    let mut fields = vec![
        ("s_keyword", query),
        ("action", "advanced_search"),
        ("page", page_s.as_str()),
    ];
    if !orderby.is_empty() {
        fields.push(("orderby", orderby));
        fields.push(("order", "DESC"));
    }
    let body = client::<C>(C::BASE_URL)
        .post(format!("{}/wp-admin/admin-ajax.php", C::BASE_URL))
        .xhr()
        .form(&fields)
        .send_text()
        .unwrap_or_else(|_| ANIMECORE_SEARCH_FIXTURE.to_string());
    let Ok(data) = serde_json::from_str::<AnimeCoreSearchResponse>(&body) else {
        return Paged::default();
    };
    Paged {
        entries: parse_page_cards::<C>(&data.data.html).entries,
        has_next_page: data.data.current_page < data.data.max_pages,
    }
}

fn parse_page_cards<C: PortalConfig>(body: &str) -> Paged<CatalogItem> {
    let doc = Html::parse_document(body);
    let selectors = match C::KIND {
        PortalKind::AnimesDigital => {
            "div.b_flex div.itemE > a[href], div.itemA > a[href], a[href*='/anime/a/']"
        }
        PortalKind::AnimesGames => {
            "ul.top10 > li > a[href], section.episodioItem > a[href], section.animeItem > a[href], a[href*='/animes/']"
        }
        PortalKind::AnimesOnlineVip => {
            "a.top100Item[href], div.videos div.video div.video-thumb a[href], a[href*='/video/'], a[href*='/anime/']"
        }
        PortalKind::AnimeCore => {
            "article.anime-card h3 > a.stretched-link[href], article.anime-card a[href]"
        }
    };
    let mut entries = Vec::new();
    for el in doc.select(&selector(selectors)) {
        if let Some(item) = card_from_anchor::<C>(el) {
            if !entries
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                entries.push(item);
            }
        }
    }
    Paged {
        entries,
        has_next_page: doc
            .select(&selector(
                "a.next, li.next, .pagination a, .paginacao li.next",
            ))
            .next()
            .is_some(),
    }
}

fn card_from_anchor<C: PortalConfig>(el: ElementRef<'_>) -> Option<CatalogItem> {
    let href = attr(&el, "href");
    if href.is_empty()
        || href.contains("wp-admin")
        || href.ends_with(".jpg")
        || href.ends_with(".png")
    {
        return None;
    }
    let path = item_path::<C>(&href);
    let title = attr(&el, "title").if_empty(
        &select_text(
            el,
            "span.title_anime, div.tituloEP, div.tituloAnime, h3, img",
        )
        .or_else(|| select_attr(el, "img", "alt"))
        .unwrap_or_else(|| title_from_path::<C>(&path)),
    );
    Some(CatalogItem {
        key: path.clone(),
        title: clean_title::<C>(&title),
        cover: image_from(el).map(|src| absolute_url::<C>(&src)),
        url: Some(absolute_url::<C>(&path)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    })
}

fn fetch_details<C: PortalConfig>(path: &str) -> CatalogItem {
    let body = fetch::<C>(&absolute_url::<C>(path), DETAILS_FIXTURE, C::BASE_URL);
    let doc = real_doc::<C>(&Html::parse_document(&body), &absolute_url::<C>(path));
    let root = doc.root_element();
    let title = match C::KIND {
        PortalKind::AnimesDigital => select_text(root, "div.crw div.dados h1, h1"),
        PortalKind::AnimesGames => select_text(root, "section.conteudoPost section > h1, h1"),
        PortalKind::AnimesOnlineVip => select_text(root, "div.pagina-titulo h1, h1"),
        PortalKind::AnimeCore => select_text(root, "title, h1, h3"),
    }
    .map(|v| clean_title::<C>(&v))
    .unwrap_or_else(|| title_from_path::<C>(path));
    let status = match C::KIND {
        PortalKind::AnimesDigital => select_text(root, "div.clw div.playon").unwrap_or_default(),
        PortalKind::AnimesGames => info_text(root, "Status").unwrap_or_default(),
        _ => String::new(),
    };
    CatalogItem {
        key: path_key::<C>(path),
        title,
        cover: image_from(root).map(|src| absolute_url::<C>(&src)),
        description: select_text(root, "div.sinopse, section.sinopseEp p, ul.post-infos p, section p, div#info p, p"),
        tags: root
            .select(&selector("div.genre a, div.sgeneros a, ul.post-infos li a, div.flex a.hover\\:text-white, a[rel='tag']"))
            .map(text)
            .filter(|v| !v.is_empty())
            .collect(),
        authors: info_text(root, "Autor")
            .or_else(|| info_text(root, "Diretor"))
            .into_iter()
            .collect(),
        artists: info_text(root, "Estudio")
            .or_else(|| info_text(root, "Estúdio"))
            .into_iter()
            .collect(),
        url: Some(absolute_url::<C>(path)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        status: parse_status(&status),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_episodes<C: PortalConfig>(path: &str) -> Vec<VideoEpisode> {
    if C::KIND == PortalKind::AnimeCore {
        return episodes_animecore::<C>(path);
    }
    let first_url = absolute_url::<C>(path);
    let body = fetch::<C>(&first_url, DETAILS_FIXTURE, C::BASE_URL);
    let doc = real_doc::<C>(&Html::parse_document(&body), &first_url);
    let mut out = episodes_from_doc::<C>(&doc);
    if C::KIND == PortalKind::AnimesDigital {
        let last = doc
            .select(&selector("ul.content-pagination li:nth-last-child(2) a"))
            .next()
            .map(text)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1)
            .min(20);
        for i in 2..=last {
            let body = fetch::<C>(
                &format!("{}/page/{i}", first_url.trim_end_matches('/')),
                DETAILS_FIXTURE,
                &first_url,
            );
            let page_doc = Html::parse_document(&body);
            out.extend(episodes_from_doc::<C>(&page_doc));
        }
    }
    if C::KIND == PortalKind::AnimesGames || C::KIND == PortalKind::AnimesOnlineVip {
        out.reverse();
    }
    out
}

fn episodes_from_doc<C: PortalConfig>(doc: &Html) -> Vec<VideoEpisode> {
    let sel = match C::KIND {
        PortalKind::AnimesDigital => "div.item_ep > a[href]",
        PortalKind::AnimesGames => "div.listaEp section.episodioItem > a[href]",
        PortalKind::AnimesOnlineVip => "ul.episodios li a[href]",
        PortalKind::AnimeCore => "",
    };
    doc.select(&selector(sel))
        .filter_map(|el| {
            let href = attr(&el, "href");
            if href.is_empty() {
                return None;
            }
            let title = match C::KIND {
                PortalKind::AnimesDigital => {
                    select_text(el, "div.title_anime").unwrap_or_else(|| text(el))
                }
                PortalKind::AnimesGames => {
                    select_text(el, "div.tituloEP").unwrap_or_else(|| text(el))
                }
                PortalKind::AnimesOnlineVip => attr(&el, "title").if_empty(&text(el)),
                PortalKind::AnimeCore => text(el),
            };
            let number = first_number(title.rsplit([' ', ':', '-']).next().unwrap_or(&title))
                .or_else(|| first_number(&title))
                .unwrap_or(1.0);
            Some(VideoEpisode {
                key: path_key::<C>(&href),
                title: Some(
                    clean_title::<C>(&title)
                        .if_empty(&format!("Episode {}", display_number(number))),
                ),
                episode_number: Some(number),
                url: Some(absolute_url::<C>(&href)),
                language: Some(C::LANG.to_string()),
                ..VideoEpisode::default()
            })
        })
        .collect()
}

fn episodes_animecore<C: PortalConfig>(path: &str) -> Vec<VideoEpisode> {
    let item_url = absolute_url::<C>(path);
    let body = fetch::<C>(&item_url, DETAILS_FIXTURE, C::BASE_URL);
    let doc = real_doc::<C>(&Html::parse_document(&body), &item_url);
    let Some(anime_id) = doc
        .select(&selector("#seasonContent[data-season]"))
        .next()
        .map(|el| attr(&el, "data-season"))
        .filter(|v| !v.is_empty())
    else {
        return Vec::new();
    };
    let target = format!(
        "{}/wp-admin/admin-ajax.php?action=get_episodes&anime_id={anime_id}&page=1&order=desc",
        C::BASE_URL
    );
    let body = client::<C>(&item_url)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| EPISODES_JSON_FIXTURE.to_string());
    let Ok(data) = serde_json::from_str::<AnimeCoreEpisodeResponse>(&body) else {
        return Vec::new();
    };
    data.data
        .episodes
        .into_iter()
        .map(|ep| {
            let key = path_key::<C>(&ep.url);
            let number = ep
                .meta_number
                .parse::<f32>()
                .ok()
                .or_else(|| first_number(&ep.number))
                .unwrap_or(1.0);
            VideoEpisode {
                key: key.clone(),
                title: Some(
                    ep.number
                        .if_empty(&format!("Episode {}", display_number(number))),
                ),
                episode_number: Some(number),
                url: Some(absolute_url::<C>(&key)),
                language: Some(C::LANG.to_string()),
                ..VideoEpisode::default()
            }
        })
        .collect()
}

fn streams_digital<C: PortalConfig>(
    body: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    let mut out = Vec::new();
    for el in doc.select(&selector("div#player div.tab-video iframe, div#player div.tab-video script, div#player div.tab-video a[href], div#player source")) {
        out.extend(streams_from_element::<C>(el, referer, "Animes Digital", request, 0));
    }
    out
}

fn streams_games<C: PortalConfig>(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    let Some(player_url) = doc
        .select(&selector("div.Link > a[href]"))
        .next()
        .map(|el| attr(&el, "href"))
        .filter(|v| !v.is_empty())
    else {
        return Vec::new();
    };
    let player_url = absolute_remote(&player_url, referer);
    let player_body = fetch::<C>(&player_url, "", referer);
    let player_doc = Html::parse_document(&player_body);
    if let Some(iframe) = player_doc.select(&selector("iframe[src]")).next() {
        return vec![external_stream::<C>(
            &absolute_remote(&attr(&iframe, "src"), &player_url),
            "External",
            &player_url,
        )];
    }
    if let Some(script) = player_doc
        .select(&selector("script"))
        .find(|s| text_or_data(*s).contains("jw"))
    {
        return streams_from_script::<C>(&text_or_data(script), &player_url, request);
    }
    Vec::new()
}

fn streams_vip<C: PortalConfig>(body: &str, referer: &str, request: &Value) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    doc.select(&selector("#video source[src], div.post-video iframe[src]"))
        .flat_map(|el| streams_from_element::<C>(el, referer, "Default", request, 0))
        .collect()
}

fn streams_animecore<C: PortalConfig>(
    body: &str,
    referer: &str,
    _request: &Value,
) -> Vec<VideoStream> {
    let doc = Html::parse_document(body);
    doc.select(&selector("div.episode-player-box iframe[src]"))
        .map(|el| {
            let src = absolute_remote(&attr(&el, "src"), referer);
            let name = if src.contains("proxycdn.cc") {
                "Proxy CDN"
            } else {
                "External"
            };
            external_stream::<C>(&src, name, referer)
        })
        .collect()
}

fn streams_from_element<C: PortalConfig>(
    el: ElementRef<'_>,
    referer: &str,
    name: &str,
    request: &Value,
    depth: usize,
) -> Vec<VideoStream> {
    if depth > 2 {
        return Vec::new();
    }
    match el.value().name() {
        "source" => {
            let src = absolute_remote(&attr(&el, "src"), referer);
            vec![stream_for_url::<C>(&src, name, referer, request)]
        }
        "iframe" => {
            let src = attr(&el, "data-lazy-src").if_empty(&attr(&el, "src"));
            let src = absolute_remote(&src, referer);
            if src.contains("blogger.com") || src.contains("assistonapi.link") {
                return vec![external_stream::<C>(&src, name, referer)];
            }
            let body = fetch::<C>(&src, "", referer);
            let doc = Html::parse_document(&body);
            let nested = doc
                .select(&selector("source[src], iframe[src], script"))
                .flat_map(|child| streams_from_element::<C>(child, &src, name, request, depth + 1))
                .collect::<Vec<_>>();
            if nested.is_empty() {
                vec![external_stream::<C>(&src, name, referer)]
            } else {
                nested
            }
        }
        "script" => streams_from_script::<C>(&text_or_data(el), referer, request),
        "a" => {
            let href = attr(&el, "href");
            if href.contains("token=") {
                let token = href
                    .split("token=")
                    .nth(1)
                    .unwrap_or_default()
                    .split('&')
                    .next()
                    .unwrap_or_default();
                let body = client::<C>(referer)
                    .get("https://sabornutritivo.com/social.php")
                    .header("Cookie", &format!("token={token};"))
                    .send_text()
                    .unwrap_or_default();
                let doc = Html::parse_document(&body);
                if let Some(iframe) = doc.select(&selector("iframe[src]")).next() {
                    return vec![external_stream::<C>(
                        &absolute_remote(
                            &attr(&iframe, "src"),
                            "https://sabornutritivo.com/social.php",
                        ),
                        name,
                        "https://sabornutritivo.com/social.php",
                    )];
                }
            }
            vec![external_stream::<C>(
                &absolute_remote(&href, referer),
                name,
                referer,
            )]
        }
        _ => Vec::new(),
    }
}

fn streams_from_script<C: PortalConfig>(
    script: &str,
    referer: &str,
    request: &Value,
) -> Vec<VideoStream> {
    let cleaned = script
        .replace("\\/", "/")
        .replace("\\\"", "\"")
        .replace("\\'", "'");
    let re = Regex::new(r#"(?s)(?:file|src)\s*[:=]\s*["']([^"']+)["'](?:[^{}]+?label\s*[:=]\s*["']([^"']+)["'])?|label\s*[:=]\s*["']([^"']+)["'][^{}]+?(?:file|src)\s*[:=]\s*["']([^"']+)["']"#).unwrap();
    let mut out = Vec::new();
    for caps in re.captures_iter(&cleaned) {
        let src = caps
            .get(1)
            .or_else(|| caps.get(4))
            .map(|m| m.as_str())
            .unwrap_or_default();
        if src.is_empty()
            || (!src.starts_with("http") && !src.starts_with("//") && !src.contains(".m3u8"))
        {
            continue;
        }
        let quality = caps
            .get(2)
            .or_else(|| caps.get(3))
            .map(|m| m.as_str())
            .unwrap_or("Default");
        out.push(stream_for_url::<C>(
            &absolute_remote(src, referer),
            quality,
            referer,
            request,
        ));
    }
    if out.is_empty() {
        for src in cleaned
            .split(['"', '\''])
            .filter(|part| part.contains(".m3u8") || part.contains(".mp4"))
        {
            out.push(stream_for_url::<C>(
                &absolute_remote(src, referer),
                "Default",
                referer,
                request,
            ));
        }
    }
    out
}

fn stream_for_url<C: PortalConfig>(
    src: &str,
    name: &str,
    referer: &str,
    request: &Value,
) -> VideoStream {
    let is_hls = src.contains(".m3u8");
    let quality = quality_from(src)
        .or_else(|| quality_from(name))
        .unwrap_or_else(|| preference(request, "preferred_quality", "720p"));
    VideoStream {
        url: src.to_string(),
        name: Some(format!("{name} - {quality}")),
        quality: Some(quality.clone()),
        format: Some(if is_hls { "hls" } else { "mp4" }.to_string()),
        is_hls,
        stream_kind: Some(if is_hls {
            VideoStreamKind::Hls
        } else {
            VideoStreamKind::Direct
        }),
        headers: referer_headers(referer),
        preferred: quality == preference(request, "preferred_quality", "720p"),
        initialized: true,
        ..VideoStream::default()
    }
}

fn external_stream<C: PortalConfig>(src: &str, name: &str, referer: &str) -> VideoStream {
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

fn real_doc<C: PortalConfig>(doc: &Html, current_url: &str) -> Html {
    let follow_selector = match C::KIND {
        PortalKind::AnimesGames => "div.linksEP > a:has(li.episodio)[href]",
        PortalKind::AnimesOnlineVip => "div.post-botoes ul li a:has(i.fa-bars)[href]",
        PortalKind::AnimeCore => "div.anime-information h4 a[href]",
        PortalKind::AnimesDigital => "",
    };
    if follow_selector.is_empty() {
        return Html::parse_document(&doc.root_element().html());
    }
    if let Some(url) = doc
        .select(&selector(follow_selector))
        .next()
        .map(|el| attr(&el, "href"))
        .filter(|v| !v.is_empty())
    {
        return Html::parse_document(&fetch::<C>(
            &absolute_remote(&url, current_url),
            DETAILS_FIXTURE,
            current_url,
        ));
    }
    Html::parse_document(&doc.root_element().html())
}

fn item_path<C: PortalConfig>(href: &str) -> String {
    if C::KIND == PortalKind::AnimeCore {
        if let Some(caps) = Regex::new(r"/watch/([^/]+)-episodio-\d+/?")
            .unwrap()
            .captures(href)
        {
            return format!("/anime/{}", caps.get(1).unwrap().as_str());
        }
    }
    path_key::<C>(href)
}

fn path_from_url<C: PortalConfig>(input: &str) -> Option<String> {
    (input.starts_with(C::BASE_URL) || input.starts_with('/')).then(|| path_key::<C>(input))
}

fn request_key<C: PortalConfig>(request: &Value, field: &str) -> Option<String> {
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

fn path_key<C: PortalConfig>(input: &str) -> String {
    if input.starts_with("http") && !input.starts_with(C::BASE_URL) {
        return input.to_string();
    }
    let without_base = input.strip_prefix(C::BASE_URL).unwrap_or(input);
    format!(
        "/{}",
        without_base
            .split('#')
            .next()
            .unwrap_or(without_base)
            .trim_matches('/')
    )
}

fn absolute_url<C: PortalConfig>(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(C::BASE_URL, input)
    }
}

fn absolute_remote(input: &str, base: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else if input.starts_with("//") {
        format!("https:{input}")
    } else {
        let root = base.rsplit_once('/').map(|(root, _)| root).unwrap_or(base);
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }
}

fn title_from_path<C: PortalConfig>(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(C::NAME)
        .replace('-', " ")
}

fn clean_title<C: PortalConfig>(input: &str) -> String {
    let mut out = input
        .replace("Assistir ", "")
        .replace("Temporada Online", "")
        .replace(" - Anime Core", "")
        .replace("Todos Episodios Assistir Online", "")
        .replace("Assistir Online", "")
        .trim()
        .to_string();
    if C::KIND == PortalKind::AnimesOnlineVip {
        out = out.rsplit('–').next().unwrap_or(&out).trim().to_string();
    }
    out
}

fn image_from(el: ElementRef<'_>) -> Option<String> {
    select_attr(el, "img", "data-lazy-src")
        .or_else(|| select_attr(el, "img", "data-src"))
        .or_else(|| {
            select_attr(el, "img", "srcset")
                .map(|v| v.split_whitespace().next().unwrap_or("").to_string())
        })
        .or_else(|| select_attr(el, "img", "src"))
        .filter(|v| !v.is_empty())
        .map(|v| v.split("?resize").next().unwrap_or(&v).to_string())
}

fn info_text(el: ElementRef<'_>, label: &str) -> Option<String> {
    let wanted = normalize(label);
    for row in el.select(&selector("li, div, span")) {
        let value = text(row);
        if normalize(&value).contains(&wanted) {
            let cleaned = value
                .replace(label, "")
                .replace(':', "")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn parse_status(input: &str) -> ItemStatus {
    match normalize(input).as_str() {
        value if value.contains("completo") => ItemStatus::Completed,
        value if value.contains("lancamento") || value.contains("em lancamento") => {
            ItemStatus::Ongoing
        }
        _ => ItemStatus::Unknown,
    }
}

fn sort_streams(streams: &mut [VideoStream], request: &Value) {
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

fn referer_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

fn selector(input: &str) -> Selector {
    Selector::parse(input).unwrap()
}

fn attr(el: &ElementRef<'_>, name: &str) -> String {
    el.value().attr(name).unwrap_or_default().to_string()
}

fn text(el: ElementRef<'_>) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn text_or_data(el: ElementRef<'_>) -> String {
    let data = el
        .children()
        .filter_map(|child| child.value().as_text())
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if data.is_empty() { text(el) } else { data }
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
        .map(|e| attr(&e, name))
        .filter(|v| !v.is_empty())
}

fn first_number(input: &str) -> Option<f32> {
    Regex::new(r"\d+(?:\.\d+)?")
        .ok()?
        .find(input)?
        .as_str()
        .parse()
        .ok()
}

fn display_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as u32)
    } else {
        value.to_string()
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

fn quality_from(input: &str) -> Option<String> {
    Regex::new(r"(?i)(\d{3,4}p|full\s*hd|hd|sd)")
        .ok()?
        .find(input)
        .map(|m| m.as_str().replace(' ', "").to_uppercase())
}

fn quality_score(input: &str) -> u32 {
    first_number(input)
        .map(|v| v as u32)
        .unwrap_or(match input {
            "FULLHD" => 1080,
            "HD" => 720,
            "SD" => 480,
            _ => 0,
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

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|f| f.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .or_else(|| request.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut cloned = request.clone();
    if let Value::Object(ref mut map) = cloned {
        map.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    cloned
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[derive(Deserialize)]
struct ListAnimeResponse {
    results: Vec<String>,
    page: u64,
    total_page: u64,
}

#[derive(Deserialize)]
struct AnimeCoreSearchResponse {
    data: AnimeCoreSearchData,
}

#[derive(Deserialize)]
struct AnimeCoreSearchData {
    html: String,
    max_pages: u64,
    current_page: u64,
}

#[derive(Deserialize)]
struct AnimeCoreEpisodeResponse {
    data: AnimeCoreEpisodeData,
}

#[derive(Deserialize)]
struct AnimeCoreEpisodeData {
    episodes: Vec<AnimeCoreEpisode>,
}

#[derive(Deserialize)]
struct AnimeCoreEpisode {
    number: String,
    url: String,
    #[serde(default)]
    meta_number: String,
}

const LIST_FIXTURE: &str = r#"<article class="anime-card"><h3><a class="stretched-link" href="/anime/sample" title="Sample Anime">Sample Anime</a></h3><img src="/poster.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Anime</h1><p>Sample details.</p><div id="seasonContent" data-season="1"></div><ul class="episodios"><li><a href="/watch/sample-episodio-1" title="Episode 1">Episode 1</a></li></ul>"#;
const PLAYER_FIXTURE: &str = r#"<div class="episode-player-box"><iframe src="https://example.invalid/embed"></iframe></div>"#;
const LIST_JSON_FIXTURE: &str = r#"{"results":["<section class=\"animeItem\"><a href=\"/animes/sample\"><div class=\"tituloAnime\">Sample Anime</div><img src=\"/poster.jpg\"></a></section>"],"page":1,"total_page":1}"#;
const ANIMECORE_SEARCH_FIXTURE: &str = r#"{"success":true,"data":{"html":"<article class=\"anime-card\"><h3><a class=\"stretched-link\" href=\"/anime/sample\" title=\"Sample Anime\">Sample Anime</a></h3><img src=\"/poster.jpg\"></article>","max_pages":1,"current_page":1}}"#;
const EPISODES_JSON_FIXTURE: &str = r#"{"success":true,"data":{"episodes":[{"number":"Episode 1","title":"Sample","released":"","url":"/watch/sample-episodio-1","meta_number":"1"}],"max_episodes_page":1}}"#;
