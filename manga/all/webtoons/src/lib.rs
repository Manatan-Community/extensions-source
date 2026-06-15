use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: Webtoons = Webtoons;
const BASE_URL: &str = "https://www.webtoons.com";
const MOBILE_URL: &str = "https://m.webtoons.com";

struct Webtoons;

impl MangaSource for Webtoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            let day = request
                .get("preferences")
                .and_then(|prefs| prefs.get("latestDay"))
                .and_then(Value::as_str)
                .unwrap_or("monday");
            format!(
                "{BASE_URL}/{}/originals/{day}?sortOrder=UPDATE",
                config.lang_code
            )
        } else {
            let rank = match page {
                1 => "trending",
                2 => "popular",
                3 => "originals",
                4 => "canvas",
                _ => "canvas",
            };
            format!("{BASE_URL}/{}/ranking/{rank}", config.lang_code)
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(Paged {
            entries: parse_listing(&body, config),
            has_next_page: listing != "latest" && page < 4,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with(MOBILE_URL) {
            let key = normalize_key(query);
            if let Some(title_no) = title_no_from_key(&key) {
                let body = fetch_document_or_fixture(
                    &url::join_url(BASE_URL, &key),
                    DETAILS_FIXTURE,
                    BASE_URL,
                );
                return Ok(Paged {
                    entries: vec![parse_details(&body, &key, config, Some(title_no))],
                    has_next_page: false,
                });
            }
        }
        if let Some(id_query) = query.strip_prefix("id:") {
            let mut parts = id_query.split(':');
            let source_type = parts.next().unwrap_or("webtoon");
            let lang = parts.next().unwrap_or(config.lang_code);
            let title_no = parts.next().unwrap_or_default();
            if lang != config.lang_code || title_no.is_empty() {
                return Ok(Paged::default());
            }
            let key = if source_type == "canvas" {
                format!("/challenge/episodeList?titleNo={title_no}")
            } else {
                format!("/episodeList?titleNo={title_no}")
            };
            let body = fetch_document_or_fixture(
                &url::join_url(BASE_URL, &key),
                DETAILS_FIXTURE,
                BASE_URL,
            );
            return Ok(Paged {
                entries: vec![parse_details(
                    &body,
                    &key,
                    config,
                    Some(title_no.to_string()),
                )],
                has_next_page: false,
            });
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        let search_type = filters
            .get("searchType")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut target = format!("{BASE_URL}/{}/search", config.lang_code);
        if !search_type.is_empty() {
            target.push('/');
            target.push_str(search_type);
        }
        target.push_str(&format!("?keyword={}", url::query_escape(query)));
        if page > 1 && !search_type.is_empty() {
            target.push_str(&format!("&page={page}"));
        }
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE, BASE_URL);
        Ok(Paged {
            entries: parse_listing(&body, config),
            has_next_page: body.contains("pagination")
                && body.contains("aria-current=true")
                && body.contains("<a"),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        let body =
            fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE, BASE_URL);
        Ok(parse_details(&body, &key, config, title_no_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        let title_no = title_no_from_key(&key).unwrap_or_else(|| "1".to_string());
        let source_type = source_type_from_key(&key);
        let target =
            format!("{MOBILE_URL}/api/v1/{source_type}/{title_no}/episodes?pageSize=99999");
        let body = fetch_json_or_fixture(&target, EPISODES_FIXTURE);
        Ok(parse_episodes(
            &body,
            preferences_bool(&request, "useSequentialNumbering"),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/viewer?title_no=1".into());
        let body =
            fetch_document_or_fixture(&url::join_url(MOBILE_URL, &key), PAGES_FIXTURE, MOBILE_URL);
        let mut pages = parse_pages(&body, preferences_bool(&request, "useMaxQuality"));
        if pages.is_empty() {
            pages = fetch_motion_toon_pages(&body);
        }
        if preferences_bool(&request, "showAuthorsNotes") {
            if let Some(note) = author_note(&body) {
                pages.push(manga::text_page(&note));
            }
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(merge_request(&request, "popular"))?;
        let latest = self.list(merge_request(&request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(MOBILE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with(MOBILE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    DETAILS_FIXTURE,
                    &key,
                    config_for(&request),
                    title_no_from_key(&key),
                )),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    lang_code: &'static str,
    locale_cookie: &'static str,
}

fn config_for(request: &Value) -> SourceConfig {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("webtoons-id") => SourceConfig {
            id: "webtoons-id",
            lang: "id",
            lang_code: "id",
            locale_cookie: "id",
        },
        Some("webtoons-th") => SourceConfig {
            id: "webtoons-th",
            lang: "th",
            lang_code: "th",
            locale_cookie: "th",
        },
        Some("webtoons-es") => SourceConfig {
            id: "webtoons-es",
            lang: "es",
            lang_code: "es",
            locale_cookie: "es",
        },
        Some("webtoons-fr") => SourceConfig {
            id: "webtoons-fr",
            lang: "fr",
            lang_code: "fr",
            locale_cookie: "fr",
        },
        Some("webtoons-zh-hant") => SourceConfig {
            id: "webtoons-zh-hant",
            lang: "zh-Hant",
            lang_code: "zh-hant",
            locale_cookie: "zh_TW",
        },
        Some("webtoons-de") => SourceConfig {
            id: "webtoons-de",
            lang: "de",
            lang_code: "de",
            locale_cookie: "de",
        },
        _ => SourceConfig {
            id: "webtoons-en",
            lang: "en",
            lang_code: "en",
            locale_cookie: "en",
        },
    }
}

fn client(referer: &str, config: Option<SourceConfig>) -> HttpClient {
    let config = config.unwrap_or_else(|| config_for(&Value::Null));
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{referer}/"))
        .with_cookies_for(BASE_URL)
        .with_header(
            "Cookie",
            format!(
                "ageGatePass=true; locale={}; needGDPR=false",
                config.locale_cookie
            ),
        )
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str, referer: &str) -> String {
    client(referer, None)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client(MOBILE_URL, None)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, config: SourceConfig) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let title = html::text_between(chunk, "class=\"title", "</")
                .or_else(|| html::text_between(chunk, "class='title", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Webtoon".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(config.lang.to_string()),
                content_rating: Some("safe".to_string()),
                extra: [("sourceId".to_string(), Value::String(config.id.to_string()))]
                    .into_iter()
                    .collect(),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(
    body: &str,
    key: &str,
    config: SourceConfig,
    title_no: Option<String>,
) -> CatalogItem {
    let title = html::text_between(body, "<h1", "</h1>")
        .or_else(|| html::text_between(body, "<h3", "</h3>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Webtoon".into()));
    let info = html::text_between(body, "detail_header", "</div>").unwrap_or_default();
    let aside = html::text_between(body, "_asideDetail", "</div>").unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title,
        authors: author_values(&info),
        artists: author_values(&info),
        tags: class_texts(&info, "genre"),
        description: html::text_between(&aside, "summary", "</")
            .map(|value| html::strip_tags(&value)),
        status: status_from_text(&aside),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "property='og:image'", "content"))
            .or_else(|| html::attr_after(body, "detail_header", "src")),
        url: Some(url::join_url(BASE_URL, key)),
        language: Some(config.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        extra: [
            ("sourceId".to_string(), Value::String(config.id.to_string())),
            (
                "titleNo".to_string(),
                Value::String(title_no.unwrap_or_default()),
            ),
        ]
        .into_iter()
        .collect(),
        ..CatalogItem::default()
    }
}

fn parse_episodes(body: &str, sequential: bool) -> Vec<MangaChapter> {
    let Ok(response) = serde_json::from_str::<EpisodeListResponse>(body) else {
        return Vec::new();
    };
    let mut episodes = response.result.episode_list;
    let mut recognized = 0;
    for episode in &mut episodes {
        if let Some(number) = episode_number(&episode.episode_title) {
            episode.chapter_number = number;
            recognized += 1;
        }
    }
    if sequential || recognized * 2 < episodes.len() {
        for (index, episode) in episodes.iter_mut().enumerate() {
            episode.chapter_number = index as f32 + 1.0;
        }
    }
    episodes
        .into_iter()
        .rev()
        .map(|episode| MangaChapter {
            key: normalize_key(&episode.viewer_link),
            title: Some(format!(
                "{} (ch. {:.2}){}",
                html::html_unescape(&episode.episode_title),
                episode.chapter_number,
                if episode.has_bgm { " music" } else { "" }
            )),
            chapter_number: Some(episode.chapter_number),
            date_uploaded: Some(episode.exposure_date_millis / 1000),
            url: Some(url::join_url(MOBILE_URL, &episode.viewer_link)),
            ..MangaChapter::default()
        })
        .collect()
}

fn parse_pages(body: &str, use_max_quality: bool) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-url"))
        .filter_map(|chunk| html::attr(chunk, "data-url"))
        .enumerate()
        .map(|(index, image)| {
            let image = if use_max_quality {
                remove_query_pair(&image, "type", "q90")
            } else {
                image
            };
            MangaPage {
                content: PageContent::Url {
                    url: image.clone(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn fetch_motion_toon_pages(body: &str) -> Vec<MangaPage> {
    let Some(doc_url) = quoted_after(body, "documentURL:") else {
        return Vec::new();
    };
    let Some(path_prefix) = quoted_after(body, "jpg:") else {
        return Vec::new();
    };
    let body = fetch_json_or_fixture(&doc_url, MOTION_FIXTURE);
    let Ok(response) = serde_json::from_str::<MotionToonResponse>(&body) else {
        return Vec::new();
    };
    response
        .assets
        .images
        .into_iter()
        .filter(|(key, _)| key.contains("layer"))
        .enumerate()
        .map(|(index, (_, image))| MangaPage {
            content: PageContent::Url {
                url: format!("{path_prefix}{image}"),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn author_note(body: &str) -> Option<String> {
    let note = html::text_between(body, "author_text", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    let creator = html::text_between(body, "author_name", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "creator".to_string());
    Some(format!("Author's Notes from {creator}\n\n{note}"))
}

fn author_values(info: &str) -> Vec<String> {
    let values = class_texts(info, "author");
    if values.is_empty() {
        class_texts(info, "author_area")
    } else {
        values
    }
}

fn class_texts(body: &str, class: &str) -> Vec<String> {
    body.split('<')
        .filter(|chunk| chunk.contains(class))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from_text(text: &str) -> ItemStatus {
    let upper = html::strip_tags(text).to_uppercase();
    if upper.contains("END") || upper.contains("COMPLETED") || upper.contains("TERMINE") {
        ItemStatus::Completed
    } else if upper.contains("UP") || upper.contains("EVERY") || upper.contains("NOUVEAU") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn episode_number(title: &str) -> Option<f32> {
    let lower = title.to_lowercase();
    for marker in ["episode", "ep.", "ep ", "chapter", "ch.", "ch "] {
        if let Some(rest) = lower.split(marker).nth(1) {
            let digits = rest
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>();
            if let Ok(number) = digits.parse::<f32>() {
                return Some(number);
            }
        }
    }
    None
}

fn normalize_key(value: &str) -> String {
    let without_domain = value
        .trim_start_matches(BASE_URL)
        .trim_start_matches(MOBILE_URL);
    format!("/{}", without_domain.trim_start_matches('/'))
}

fn title_no_from_key(key: &str) -> Option<String> {
    key.split('?').nth(1)?.split('&').find_map(|part| {
        part.strip_prefix("title_no=")
            .or_else(|| part.strip_prefix("titleNo="))
            .map(ToString::to_string)
    })
}

fn source_type_from_key(key: &str) -> &'static str {
    if key.contains("/canvas/") || key.contains("/challenge/") {
        "canvas"
    } else {
        "webtoon"
    }
}

fn remove_query_pair(input: &str, key: &str, value: &str) -> String {
    let Some((base, query)) = input.split_once('?') else {
        return input.to_string();
    };
    let kept = query
        .split('&')
        .filter(|part| *part != format!("{key}={value}"))
        .collect::<Vec<_>>();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    for quote in ['\'', '"'] {
        let start = rest.find(quote)? + 1;
        let after = &rest[start..];
        let end = after.find(quote)?;
        let value = after[..end].split('{').next().unwrap_or_default();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn preferences_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn merge_request(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    if let Value::Object(ref mut map) = value {
        map.insert("listingId".to_string(), Value::String(listing.to_string()));
    }
    value
}

fn sample_key() -> String {
    "/en/fantasy/sample/list?title_no=1".to_string()
}

#[derive(Deserialize)]
struct EpisodeListResponse {
    result: EpisodeList,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeList {
    episode_list: Vec<Episode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    episode_title: String,
    viewer_link: String,
    exposure_date_millis: i64,
    #[serde(default)]
    has_bgm: bool,
    #[serde(skip)]
    chapter_number: f32,
}

#[derive(Deserialize)]
struct MotionToonResponse {
    assets: MotionToonAssets,
}

#[derive(Deserialize)]
struct MotionToonAssets {
    images: BTreeMap<String, String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<ul class="webtoon_list">
  <li><a href="https://www.webtoons.com/en/fantasy/sample/list?title_no=1"><span class="title">Sample Toon</span><img src="https://img.example/cover.jpg"></a></li>
</ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<html><head><meta property="og:image" content="https://img.example/cover.jpg"></head>
<body>
<div class="detail_header"><div class="info"><h1 class="subj">Sample Toon</h1><span class="author">Writer</span><span class="author">Artist</span><span class="genre">Fantasy</span></div></div>
<div id="_asideDetail"><p class="summary">A sample story.</p><p class="day_info">EVERY MONDAY</p></div>
</body></html>
"#;

const EPISODES_FIXTURE: &str = r#"{"result":{"episodeList":[{"episodeTitle":"Episode 1","viewerLink":"/en/fantasy/sample/ep-1/viewer?title_no=1&episode_no=1","exposureDateMillis":1704067200000,"hasBgm":true}]}}"#;

const PAGES_FIXTURE: &str = r#"
<div id="_imageList"><img data-url="https://img.example/page-1.jpg?type=q90"></div>
<div class="creator_note"><p class="author_text">Thanks for reading.</p><div class="author_name"><span>Writer</span></div></div>
"#;

const MOTION_FIXTURE: &str = r#"{"assets":{"images":{"layer1":"1.jpg","background":"bg.jpg"}}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_and_episodes() {
        let config = config_for(&Value::Null);
        assert_eq!(parse_listing(LIST_FIXTURE, config).len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, &sample_key(), config, Some("1".into())).title,
            "Sample Toon"
        );
        let chapters = parse_episodes(EPISODES_FIXTURE, false);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }

    #[test]
    fn parses_pages_and_author_notes() {
        let pages = parse_pages(PAGES_FIXTURE, true);
        assert_eq!(pages.len(), 1);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://img.example/page-1.jpg"),
            _ => panic!("expected URL page"),
        }
        assert!(
            author_note(PAGES_FIXTURE)
                .unwrap()
                .contains("Thanks for reading")
        );
    }

    #[test]
    fn parses_motion_toon_paths() {
        let fixture =
            "documentURL: 'https://assets.example/doc.json', jpg: 'https://cdn.example/{'";
        let pages = fetch_motion_toon_pages(fixture);
        assert_eq!(pages.len(), 1);
    }
}
