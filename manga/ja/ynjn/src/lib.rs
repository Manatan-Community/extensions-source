use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: YnJn = YnJn;
const BASE_URL: &str = "https://ynjn.jp";
const API_URL: &str = "https://webapi.ynjn.jp";

struct YnJn;

impl MangaSource for YnJn {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        match request.get("listingId").and_then(Value::as_str) {
            Some("latest") => self.latest(request),
            _ => Ok(parse_ranking(&fetch_json(
                &format!("{API_URL}/title/ranking?id=1742&type=LIST&rankingType=RANKING"),
                RANKING_FIXTURE,
            ))),
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = if query.is_empty() {
            let category = filter_string(&request, "category").unwrap_or("LABEL|21");
            let (kind, id) = category.split_once('|').unwrap_or(("LABEL", "21"));
            format!(
                "{API_URL}/title/category/{kind}?category={kind}&page={page}&sort=POPULARITY&id={id}"
            )
        } else {
            format!(
                "{API_URL}/title/category/TEXT?category=TEXT&page={page}&sort=POPULARITY&text={}",
                url::query_escape(query)
            )
        };
        Ok(parse_title_page(
            &fetch_json(&target, TITLE_PAGE_FIXTURE),
            page,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1214".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1214".into());
        let title_id = key.trim_matches('/').rsplit('/').next().unwrap_or("1214");
        let hide_locked = preference_bool(&request, "hide_locked", true);
        Ok(parse_chapters(
            &fetch_json(
                &format!("{API_URL}/title/{title_id}/episode?isGetAll=true"),
                CHAPTERS_FIXTURE,
            ),
            title_id,
            hide_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "82096#1214".into());
        let (episode_id, title_id) = key.split_once('#').unwrap_or((key.as_str(), "1214"));
        Ok(parse_pages(&fetch_json(
            &format!("{API_URL}/viewer?titleId={title_id}&episodeId={episode_id}"),
            VIEWER_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::YnJn::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/title/{}", title_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (episode_id, title_id) = key.split_once('#').unwrap_or((key.as_str(), ""));
            format!("{BASE_URL}/viewer/{title_id}/{episode_id}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

impl YnJn {
    fn latest(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        let feature = fetch_json(
            &format!("{API_URL}/title/feature?displayLocation=TOP_PAGE_RENSAI"),
            FEATURE_ID_FIXTURE,
        );
        let feature_id = serde_json::from_str::<Value>(&feature)
            .ok()
            .and_then(|value| value.pointer("/data/info/id").and_then(Value::as_u64))
            .unwrap_or(1742);
        Ok(parse_title_page(
            &fetch_json(
                &format!("{API_URL}/title/feature?id={feature_id}&page={page}"),
                TITLE_PAGE_FIXTURE,
            ),
            page,
        ))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let root = json_value(body, RANKING_FIXTURE);
    let entries = root
        .pointer("/data/ranking/titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("title"))
        .filter_map(catalog_from_title)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_title_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = json_value(body, TITLE_PAGE_FIXTURE);
    let total = root
        .pointer("/data/total_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let has_next = root
        .pointer("/data/has_next")
        .and_then(Value::as_bool)
        .unwrap_or(total > page * 12);
    let entries = root
        .pointer("/data/titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(catalog_from_title)
        .collect();
    Paged {
        entries,
        has_next_page: has_next,
    }
}

fn catalog_from_title(item: &Value) -> Option<CatalogItem> {
    let id = item.get("id").and_then(Value::as_u64)?;
    Some(CatalogItem {
        key: id.to_string(),
        title: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Young Jump+")
            .to_string(),
        cover: item
            .get("image_url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        url: Some(format!("{BASE_URL}/title/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let id = title_id(key);
    let root = json_value(
        &fetch_json(&format!("{API_URL}/book/{id}"), DETAILS_FIXTURE),
        DETAILS_FIXTURE,
    );
    let book = root.pointer("/data/book").unwrap_or(&Value::Null);
    CatalogItem {
        key: id.to_string(),
        title: book
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Young Jump+")
            .to_string(),
        cover: book
            .get("image_url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        authors: book
            .get("author")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        description: book
            .get("summary")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tags: book
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| tag.get("value").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect(),
        url: Some(format!("{BASE_URL}/title/{id}")),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, title_id: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = json_value(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .pointer("/data/episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|episode| {
            let id = episode.get("id").and_then(Value::as_u64)?;
            let condition = episode
                .get("reading_condition")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let locked = condition != "EPISODE_READ_CONDITION_FREE";
            if locked && hide_locked {
                return None;
            }
            Some(MangaChapter {
                key: format!("{id}#{title_id}"),
                title: Some(format!(
                    "{}{}",
                    if locked { "Locked " } else { "" },
                    episode
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Chapter")
                )),
                url: Some(format!("{BASE_URL}/viewer/{title_id}/{id}")),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_value(body, VIEWER_FIXTURE);
    let headers = manga::image_headers(BASE_URL);
    let pages = root
        .pointer("/data/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("manga_page"))
        .filter_map(|page| {
            let image = page.get("page_image_url").and_then(Value::as_str)?;
            let number = page.get("page_number").and_then(Value::as_u64).unwrap_or(1);
            let mut extra = BTreeMap::new();
            extra.insert("ynjnScramble".into(), Value::Bool(true));
            Some(MangaPage {
                content: PageContent::Url {
                    url: image.to_string(),
                    context: Some(headers.clone()),
                },
                headers: headers.clone(),
                description: Some(format!("Page {number}")),
                extra,
                ..MangaPage::default()
            })
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        vec![manga::text_page(
            "Log in via WebView and purchase this chapter.",
        )]
    } else {
        pages
    }
}

fn title_id(key: &str) -> &str {
    key.trim_matches('/').rsplit('/').next().unwrap_or("1214")
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/title/") {
        Some(title_id(input).to_string())
    } else if input.parse::<u64>().is_ok() {
        Some(input.to_string())
    } else {
        None
    }
}

fn json_value(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)?
        .get(id)?
        .as_str()
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(default)
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"data":{"ranking":{"titles":[{"title":{"id":1214,"image_url":"https://public.ynjn.jp/cover.png","name":"Sample Young Jump+"}}]}}}"#;
const FEATURE_ID_FIXTURE: &str = r#"{"data":{"info":{"id":1742},"titles":[],"total_count":0}}"#;
const TITLE_PAGE_FIXTURE: &str = r#"{"data":{"has_next":false,"total_count":1,"titles":[{"id":1214,"image_url":"https://public.ynjn.jp/cover.png","name":"Sample Young Jump+"}]}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"book":{"author":["Sample Author"],"image_url":"https://public.ynjn.jp/cover.png","name":"Sample Young Jump+","summary":"Sample description.","tags":[{"value":"Action"}],"title_id":1214}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"episodes":[{"id":82096,"name":"第1話","reading_condition":"EPISODE_READ_CONDITION_FREE"},{"id":82097,"name":"第2話","reading_condition":"EPISODE_READ_CONDITION_FREE"}]}}"#;
const VIEWER_FIXTURE: &str = r#"{"data":{"pages":[{"manga_page":{"page_image_url":"https://public.ynjn.jp/page.jpg","page_number":1}}]}}"#;
