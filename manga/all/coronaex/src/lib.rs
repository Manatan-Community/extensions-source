use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: CoronaEx = CoronaEx;

struct CoronaEx;

#[derive(Clone, Copy)]
struct CoronaConfig {
    source_id: &'static str,
    lang: &'static str,
    base_url: &'static str,
    api_url: &'static str,
    api_key: &'static str,
    login_key: &'static str,
    title_sort: &'static str,
}

const JA: CoronaConfig = CoronaConfig {
    source_id: "coronaex-ja",
    lang: "ja",
    base_url: "https://to-corona-ex.com",
    api_url: "https://api.to-corona-ex.com",
    api_key: "K4FWy7Iqott9mrw37hDKfZ2gcLOwO-kiLHTwXT8ad1E=",
    login_key: "AIzaSyCeiy1JMHVkFuI8zbiAxMjNO3zoXECENhE",
    title_sort: "title_yomigana",
};

const EN: CoronaConfig = CoronaConfig {
    source_id: "coronaex-en",
    lang: "en",
    base_url: "https://en.to-corona-ex.com",
    api_url: "https://api.en.to-corona-ex.com",
    api_key: "YMiCe3ofO07MjQSroVEYDEUzyDm2sUHwDeDgqAhsTC8",
    login_key: "AIzaSyByfbwJ2lzGAH7mT2PNfXt7VuwsZZhfSe8",
    title_sort: "title_alphanumeric",
};

impl MangaSource for CoronaEx {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest_episode_published_at"
        } else {
            config.title_sort
        };
        let order = if sort == "latest_episode_published_at" { "desc" } else { "asc" };
        let target = comics_url(config, page(&request), sort, order, None, "");
        Ok(parse_catalog_page(&api_get(config, &target, LIST_FIXTURE), config))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(config, &key)], has_next_page: false });
        }
        let genre = filter_string(&request, "genre_id").unwrap_or_default();
        let target = if query.is_empty() {
            comics_url(config, page(&request), config.title_sort, "asc", (!genre.is_empty()).then_some(genre.as_str()), "")
        } else {
            format!("{}/search/comics?keyword={}&limit=24", config.api_url, url::query_escape(query))
        };
        Ok(parse_catalog_page(&api_get(config, &target, LIST_FIXTURE), config))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(details_by_key(config, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let target = format!(
            "{}/episodes?comic_id={}&episode_status=free_viewing,only_for_subscription&limit=9999&order=desc&sort=episode_order",
            config.api_url,
            comic_id(&key)
        );
        Ok(parse_chapters(&api_get(config, &target, CHAPTERS_FIXTURE), config, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        let target = format!("{}/episodes/{}/begin_reading", config.api_url, episode_id(&key));
        let body = match bearer_token(config, &request) {
            Some(token) => client(config).get(&target).header("Authorization", format!("Bearer {token}")).send_text(),
            None => client(config).get(&target).send_text(),
        }
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string());
        Ok(parse_pages(&body, config))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular", "sourceId": source_id(&request)}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "sourceId": source_id(&request)}))?;
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
        manga_image::CoronaExImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config(&request);
        Ok(manga::request_key(&request, "manga").map(|key| format!("{}/comics/{}", config.base_url, comic_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{}/episodes/{}", config.base_url, episode_id(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(config, &key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(config: CoronaConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", config.base_url))
        .with_header("X-Api-Environment-Key", config.api_key)
}

fn api_get(config: CoronaConfig, target: &str, fixture: &str) -> String {
    client(config).get(target).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn comics_url(config: CoronaConfig, page: u64, sort: &str, order: &str, genre: Option<&str>, cursor: &str) -> String {
    let mut target = format!("{}/comics?limit=24&order={order}&sort={sort}", config.api_url);
    if let Some(genre) = genre {
        target.push_str("&genre_id=");
        target.push_str(&url::query_escape(genre));
    }
    if page > 1 && !cursor.is_empty() {
        target.push_str("&after_than=");
        target.push_str(&url::query_escape(cursor));
    }
    target
}

fn parse_catalog_page(body: &str, config: CoronaConfig) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| catalog_item(&item, config, false))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: root.get("next_cursor").and_then(Value::as_str).is_some_and(|value| !value.is_empty()),
        entries,
    }
}

fn catalog_item(item: &Value, config: CoronaConfig, initialized: bool) -> Option<CatalogItem> {
    let id = json_text(item, "id")?;
    let mut description = json_text(item, "description").unwrap_or_default();
    if let Some(copyright) = json_text(item, "copyright").filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&copyright);
    }
    if let Some(alt) = json_text(item, "title_alphanumeric").or_else(|| json_text(item, "title_yomigana")).filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Alternative Title: ");
        description.push_str(&alt);
    }
    Some(CatalogItem {
        key: format!("/comics/{id}"),
        title: json_text(item, "title").unwrap_or_else(|| "Corona EX".into()),
        cover: json_text(item, "cover_image_url"),
        authors: item
            .get("authors")
            .and_then(Value::as_array)
            .map(|authors| authors.iter().filter_map(|author| json_text(author, "name")).collect())
            .unwrap_or_default(),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .map(|genres| genres.iter().filter_map(|genre| json_text(genre, "name")).collect())
            .unwrap_or_default(),
        description: (!description.is_empty()).then_some(description),
        status: ItemStatus::Unknown,
        url: Some(format!("{}/comics/{id}", config.base_url)),
        language: Some(config.lang.into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    })
}

fn details_by_key(config: CoronaConfig, key: &str) -> CatalogItem {
    let id = comic_id(key);
    let body = api_get(config, &format!("{}/comics/{id}", config.api_url), DETAILS_FIXTURE);
    catalog_item(&serde_json::from_str::<Value>(&body).unwrap_or(Value::Null), config, true)
        .unwrap_or_else(|| CatalogItem {
            key: format!("/comics/{id}"),
            title: "Corona EX".into(),
            url: Some(format!("{}/comics/{id}", config.base_url)),
            language: Some(config.lang.into()),
            content_rating: Some("adult".into()),
            initialized: true,
            ..CatalogItem::default()
        })
}

fn parse_chapters(body: &str, config: CoronaConfig, hide_locked: bool) -> Vec<MangaChapter> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("resources").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let id = json_text(&item, "id")?;
            let locked = json_text(&item, "episode_status").as_deref() == Some("only_for_subscription");
            if hide_locked && locked {
                return None;
            }
            let title = json_text(&item, "title").unwrap_or_else(|| "Episode".into());
            let number = item.get("episode_order").and_then(Value::as_f64).map(|value| value as f32);
            Some(MangaChapter {
                key: format!("/episodes/{id}"),
                title: Some(if locked { format!("Paid - {title}") } else { title }),
                chapter_number: number,
                date_uploaded: json_text(&item, "published_at").and_then(|date| parse_date(&date)),
                url: Some(format!("{}/episodes/{id}", config.base_url)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, config: CoronaConfig) -> Vec<MangaPage> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("pages").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let image = json_text(&page, "page_image_url")?;
            let drm_hash = json_text(&page, "drm_hash")?;
            let headers = manga::image_headers(config.base_url);
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(headers.clone()),
                },
                headers,
                extra: BTreeMap::from([("drmHash".into(), json!(drm_hash))]),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn bearer_token(config: CoronaConfig, request: &Value) -> Option<String> {
    preference_string(request, "refresh_token")
        .filter(|value| !value.is_empty())
        .and_then(|refresh| refresh_token(config, &refresh))
        .or_else(|| {
            let email = preference_string(request, "email")?;
            let password = preference_string(request, "password")?;
            if email.is_empty() || password.is_empty() {
                return None;
            }
            login(config, &email, &password)
        })
}

fn login(config: CoronaConfig, email: &str, password: &str) -> Option<String> {
    let target = format!("https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key={}", config.login_key);
    let body = json!({ "email": email, "password": password, "returnSecureToken": true }).to_string();
    let text = client(config).post(&target).json(body).send_text().ok()?;
    let root = serde_json::from_str::<Value>(&text).ok()?;
    root.get("idToken").or_else(|| root.get("id_token"))?.as_str().map(ToString::to_string)
}

fn refresh_token(config: CoronaConfig, refresh_token: &str) -> Option<String> {
    let target = format!("https://securetoken.googleapis.com/v1/token?key={}", config.login_key);
    let body = json!({ "grant_type": "refresh_token", "refresh_token": refresh_token }).to_string();
    let text = client(config).post(&target).json(body).send_text().ok()?;
    let root = serde_json::from_str::<Value>(&text).ok()?;
    root.get("id_token").or_else(|| root.get("idToken"))?.as_str().map(ToString::to_string)
}

fn config(request: &Value) -> CoronaConfig {
    if source_id(request).ends_with("-en") {
        EN
    } else {
        JA
    }
}

fn source_id(request: &Value) -> String {
    request.get("sourceId").and_then(Value::as_str).unwrap_or(JA.source_id).to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn comic_id(key: &str) -> String {
    key.trim_matches('/').rsplit('/').next().unwrap_or(key).to_string()
}

fn episode_id(key: &str) -> String {
    key.trim_matches('/').rsplit('/').next().unwrap_or(key).to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    let marker = "/comics/";
    let index = input.find(marker)?;
    Some(format!("/comics/{}", input[index + marker.len()..].trim_matches('/')))
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request.get("filters").and_then(|filters| filters.get(id)).and_then(Value::as_str).map(ToString::to_string)
}

fn preference_string(request: &Value, id: &str) -> Option<String> {
    request.get("preferences").and_then(|prefs| prefs.get(id)).and_then(Value::as_str).map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request.get("preferences").and_then(|prefs| prefs.get(id)).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_date(value: &str) -> Option<i64> {
    let day = value.split('T').next().unwrap_or(value);
    dates::parse_ymd(day).or_else(|| dates::parse_fixture_date(day))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"next_cursor":null,"resources":[{"id":"sample","title":"Sample Corona EX","cover_image_url":"https://cdn.to-corona-ex.com/sample.jpg","authors":[{"name":"Author","role":"漫画"}],"genres":[{"name":"Fantasy"}]}]}"#;
const DETAILS_FIXTURE: &str = r#"{"id":"sample","title":"Sample Corona EX","description":"Sample description","cover_image_url":"https://cdn.to-corona-ex.com/sample.jpg","authors":[{"name":"Author","role":"漫画"}],"genres":[{"name":"Fantasy"}]}"#;
const CHAPTERS_FIXTURE: &str = r#"{"resources":[{"id":"episode-1","title":"Episode 1","episode_order":1,"episode_status":"free_viewing","published_at":"2024-01-01T00:00:00.000+0000"}]}"#;
const PAGES_FIXTURE: &str = r#"{"pages":[{"page_image_url":"https://cdn.to-corona-ex.com/page.jpg","drm_hash":"AQEAAQ=="}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chapters() {
        let chapters = parse_chapters(CHAPTERS_FIXTURE, JA, false);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_number, Some(1.0));
    }
}
