use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeSet;

const SOURCE: Pixiv = Pixiv;
const BASE_URL: &str = "https://www.pixiv.net";
const LOCALES: [&str; 5] = ["en", "ja", "zh", "zh-tw", "ko"];

struct Pixiv;

impl MangaSource for Pixiv {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = lang_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            let target = api_url(
                "/touch/ajax/latest?type=manga",
                lang,
                &[("p", page.to_string())],
            );
            let body = fetch_json_or_fixture(&target, LATEST_FIXTURE);
            return Ok(parse_illust_results(&body, lang));
        }

        let ranking_url = api_url(
            "/touch/ajax/ranking/illust?mode=daily&type=manga",
            lang,
            &[("page", page.to_string())],
        );
        let ranking = fetch_json_or_fixture(&ranking_url, RANKING_FIXTURE);
        let ids = api_body(&ranking)
            .get("ranking")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("illustId").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Paged::default());
        }
        let params = ids
            .iter()
            .map(|id| ("illust_ids[]", id.to_string()))
            .collect::<Vec<_>>();
        let details_url = api_url("/touch/ajax/illust/details/many", lang, &params);
        let details = fetch_json_or_fixture(&details_url, MANY_DETAILS_FIXTURE);
        Ok(parse_illust_details_many(&details, lang))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = lang_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(target) = PixivTarget::parse(query) {
            return Ok(search_target(target, lang));
        }

        let filters = request.get("filters").unwrap_or(&Value::Null);
        if query.is_empty() {
            if let Some(users) = text_filter(filters, "users").filter(|value| !value.is_empty()) {
                return Ok(search_users(&users, lang, page_for(&request)));
            }
        }

        let word = if query.is_empty() {
            text_filter(filters, "tags")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "漫画".to_string())
        } else {
            query.to_string()
        };
        let mut params = vec![
            ("word", word),
            (
                "s_mode",
                if query.is_empty() {
                    text_filter(filters, "tagsMode").unwrap_or_else(|| "s_tag".to_string())
                } else {
                    "s_tc".to_string()
                },
            ),
            ("p", page_for(&request).to_string()),
        ];
        if let Some(value) = text_filter(filters, "type").filter(|value| value != "all") {
            params.push(("type", value));
        } else {
            params.push(("type", "manga".to_string()));
        }
        if let Some(value) = text_filter(filters, "rating").filter(|value| value != "all") {
            params.push(("mode", value));
        }
        if let Some(value) = text_filter(filters, "order").filter(|value| !value.is_empty()) {
            params.push(("order", value));
        }
        if let Some(value) = text_filter(filters, "postedBefore").filter(|value| !value.is_empty())
        {
            params.push(("ecd", value));
        }
        if let Some(value) = text_filter(filters, "postedAfter").filter(|value| !value.is_empty()) {
            params.push(("scd", value));
        }

        let target = api_url("/touch/ajax/search/illusts", lang, &params);
        let body = fetch_json_or_fixture(&target, SEARCH_FIXTURE);
        let mut page = parse_illust_results(&body, lang);
        apply_post_filters(&mut page.entries, filters);
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let lang = lang_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/artworks/123".into());
        Ok(match PixivTarget::parse(&url::join_url(BASE_URL, &key)) {
            Some(PixivTarget::User(user_id)) => user_details(&user_id, lang),
            Some(PixivTarget::Series { series_id, .. }) => series_details(&series_id, lang),
            Some(PixivTarget::Illustration(illust_id)) => illust_details(&illust_id, lang),
            None => catalog_item(&key, "Pixiv", None, lang),
        })
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let lang = lang_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/artworks/123".into());
        let chapters = match PixivTarget::parse(&url::join_url(BASE_URL, &key)) {
            Some(PixivTarget::User(user_id)) => user_illusts(&user_id, lang, 1)
                .into_iter()
                .map(|illust| chapter_from_illust(&illust))
                .collect(),
            Some(PixivTarget::Series { series_id, .. }) => series_illusts(&series_id, lang)
                .into_iter()
                .enumerate()
                .map(|(index, illust)| {
                    let mut chapter = chapter_from_illust(&illust);
                    chapter.chapter_number = Some((index + 1) as f32);
                    chapter
                })
                .collect(),
            Some(PixivTarget::Illustration(illust_id)) => {
                vec![chapter_from_illust(&fetch_illust(&illust_id, lang))]
            }
            None => Vec::new(),
        };
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let lang = lang_for(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/artworks/123".into());
        let illust_id = key.rsplit('/').next().unwrap_or("123");
        let target = api_url(&format!("/ajax/illust/{illust_id}/pages"), lang, &[]);
        let body = fetch_json_or_fixture(&target, PAGES_FIXTURE);
        let quality = request
            .get("preferences")
            .and_then(|prefs| prefs.get("imageQuality"))
            .and_then(Value::as_str)
            .unwrap_or("original");
        Ok(api_body(&body)
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, page)| {
                image_url(page.get("urls")?, quality).map(|image| (index, image))
            })
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let lang = lang_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(target) = PixivTarget::parse(input) {
            return Ok(Some(UrlResolveResult {
                item: search_target(target, lang).entries.into_iter().next(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum PixivTarget {
    User(String),
    Illustration(String),
    Series {
        series_id: String,
        user_id: Option<String>,
    },
}

impl PixivTarget {
    fn parse(input: &str) -> Option<Self> {
        for (prefix, parser) in [
            ("user:", PixivTarget::User as fn(String) -> PixivTarget),
            (
                "aid:",
                PixivTarget::Illustration as fn(String) -> PixivTarget,
            ),
        ] {
            if let Some(id) = input.strip_prefix(prefix).filter(|id| is_digits(id)) {
                return Some(parser(id.to_string()));
            }
        }
        if let Some(id) = input.strip_prefix("sid:").filter(|id| is_digits(id)) {
            return Some(PixivTarget::Series {
                series_id: id.to_string(),
                user_id: None,
            });
        }

        let path = input
            .strip_prefix(BASE_URL)
            .or_else(|| input.strip_prefix("https://pixiv.net"))
            .or_else(|| input.strip_prefix("http://www.pixiv.net"))
            .or_else(|| input.strip_prefix("http://pixiv.net"))
            .unwrap_or(input);
        let mut parts = path
            .trim_start_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.first().is_some_and(|part| LOCALES.contains(part)) {
            parts.remove(0);
        }
        match parts.as_slice() {
            ["artworks", id, ..] if is_digits(id) => Some(PixivTarget::Illustration((*id).into())),
            ["users", id, ..] if is_digits(id) => Some(PixivTarget::User((*id).into())),
            ["user", user_id, "series", series_id, ..]
                if is_digits(user_id) && is_digits(series_id) =>
            {
                Some(PixivTarget::Series {
                    series_id: (*series_id).into(),
                    user_id: Some((*user_id).into()),
                })
            }
            _ => None,
        }
    }
}

fn lang_for(request: &Value) -> &'static str {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("pixiv-en");
    LOCALES
        .iter()
        .copied()
        .find(|lang| id == format!("pixiv-{lang}"))
        .unwrap_or("en")
}

fn page_for(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn api_url(path: &str, lang: &str, params: &[(&str, String)]) -> String {
    let (path, existing_query) = path.split_once('?').unwrap_or((path, ""));
    let mut query = Vec::new();
    if !existing_query.is_empty() {
        query.push(existing_query.to_string());
    }
    query.extend(
        params
            .iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value))),
    );
    query.push(format!("lang={}", url::query_escape(lang)));
    format!("{BASE_URL}{path}?{}", query.join("&"))
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_body(raw: &str) -> Value {
    let value = serde_json::from_str::<Value>(raw).unwrap_or(Value::Null);
    if value.get("error").and_then(Value::as_bool) == Some(true) {
        return Value::Null;
    }
    value.get("body").cloned().unwrap_or(value)
}

fn parse_illust_results(body: &str, lang: &str) -> Paged<CatalogItem> {
    let entries = api_body(body)
        .get("illusts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|illust| illust.get("is_ad_container").and_then(Value::as_i64) != Some(1))
        .filter(|illust| illust.get("type").and_then(Value::as_str) != Some("2"))
        .map(|illust| item_from_illust(illust, lang))
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn parse_illust_details_many(body: &str, lang: &str) -> Paged<CatalogItem> {
    let entries = api_body(body)
        .get("illust_details")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|illust| item_from_illust(illust, lang))
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn item_from_illust(illust: &Value, lang: &str) -> CatalogItem {
    if let Some(series) = illust.get("series").filter(|value| !value.is_null()) {
        let mut item = item_from_series(series, lang);
        if item.cover.is_none() {
            item.cover = illust
                .get("url")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        return item;
    }
    let id = illust
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut item = catalog_item(
        &format!("/artworks/{id}"),
        illust
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled"),
        illust.get("url").and_then(Value::as_str),
        lang,
    );
    hydrate_illust_fields(&mut item, illust);
    item.initialized = false;
    item
}

fn item_from_series(series: &Value, lang: &str) -> CatalogItem {
    let id = series
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let user_id = series
        .get("userId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let cover = series.get("coverImage").and_then(|value| {
        value
            .as_str()
            .or_else(|| value.get("url").and_then(Value::as_str))
    });
    catalog_item(
        &format!("/user/{user_id}/series/{id}"),
        series
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled series"),
        cover,
        lang,
    )
}

fn catalog_item(key: &str, title: &str, cover: Option<&str>, lang: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: title.to_string(),
        cover: cover.map(ToString::to_string),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some(lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn hydrate_illust_fields(item: &mut CatalogItem, illust: &Value) {
    if let Some(author) = illust
        .get("author_details")
        .and_then(|author| author.get("user_name"))
        .and_then(Value::as_str)
    {
        item.authors = vec![author.to_string()];
        item.artists = vec![author.to_string()];
    }
    item.description = illust
        .get("comment")
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    item.tags = illust
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    item.content_rating = match illust.get("x_restrict").and_then(Value::as_str) {
        Some("0") => Some("safe".to_string()),
        Some("1") | Some("2") => Some("adult".to_string()),
        _ => item.content_rating.clone(),
    };
    item.initialized = true;
}

fn search_target(target: PixivTarget, lang: &str) -> Paged<CatalogItem> {
    let item = match target {
        PixivTarget::User(user_id) => user_details(&user_id, lang),
        PixivTarget::Illustration(illust_id) => illust_details(&illust_id, lang),
        PixivTarget::Series { series_id, .. } => series_details(&series_id, lang),
    };
    Paged {
        entries: vec![item],
        has_next_page: false,
    }
}

fn fetch_illust(illust_id: &str, lang: &str) -> Value {
    let target = api_url(
        "/touch/ajax/illust/details",
        lang,
        &[("illust_id", illust_id.to_string())],
    );
    api_body(&fetch_json_or_fixture(&target, ILLUST_FIXTURE))
        .get("illust_details")
        .cloned()
        .unwrap_or(Value::Null)
}

fn illust_details(illust_id: &str, lang: &str) -> CatalogItem {
    let illust = fetch_illust(illust_id, lang);
    let mut item = item_from_illust(&illust, lang);
    if item.key == "/artworks/unknown" {
        item.key = format!("/artworks/{illust_id}");
        item.url = Some(url::join_url(BASE_URL, &item.key));
    }
    item
}

fn series_details(series_id: &str, lang: &str) -> CatalogItem {
    let target = api_url(&format!("/touch/ajax/illust/series/{series_id}"), lang, &[]);
    let body = api_body(&fetch_json_or_fixture(&target, SERIES_FIXTURE));
    let mut item = body
        .get("series")
        .map(|series| item_from_series(series, lang))
        .unwrap_or_else(|| {
            catalog_item(
                &format!("/user/unknown/series/{series_id}"),
                "Pixiv series",
                None,
                lang,
            )
        });
    let illusts = series_illusts(series_id, lang);
    if let Some(first) = illusts.first() {
        let author = first
            .get("author_details")
            .and_then(|author| author.get("user_name"))
            .and_then(Value::as_str);
        if let Some(author) = author {
            item.authors = vec![author.to_string()];
            item.artists = vec![author.to_string()];
        }
    }
    item.description = body
        .get("series")
        .and_then(|series| series.get("caption"))
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    item.tags = illusts
        .iter()
        .flat_map(|illust| {
            illust
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if item.cover.is_none() {
        item.cover = illusts
            .first()
            .and_then(|illust| illust.get("url"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    item.initialized = true;
    item
}

fn user_details(user_id: &str, lang: &str) -> CatalogItem {
    let target = api_url(&format!("/ajax/user/{user_id}?full=1"), lang, &[]);
    let user = api_body(&fetch_json_or_fixture(&target, USER_FIXTURE));
    let name = user
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("User {user_id}"));
    let mut item = catalog_item(
        &format!("/users/{user_id}"),
        &name,
        user.get("imageBig").and_then(Value::as_str),
        lang,
    );
    item.authors = vec![name.clone()];
    item.artists = vec![name];
    item.description = user
        .get("comment")
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    item.initialized = true;
    item
}

fn user_illusts(user_id: &str, lang: &str, page: u64) -> Vec<Value> {
    let target = api_url(
        "/touch/ajax/user/illusts",
        lang,
        &[("id", user_id.to_string()), ("p", page.to_string())],
    );
    api_body(&fetch_json_or_fixture(&target, USER_ILLUSTS_FIXTURE))
        .get("illusts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn series_illusts(series_id: &str, lang: &str) -> Vec<Value> {
    let mut all = Vec::new();
    let mut last_order = 0usize;
    loop {
        let target = api_url(
            &format!("/touch/ajax/illust/series_content/{series_id}"),
            lang,
            &[("last_order", last_order.to_string())],
        );
        let body = api_body(&fetch_json_or_fixture(&target, SERIES_CONTENT_FIXTURE));
        let batch = body
            .get("series_contents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if batch.is_empty() {
            break;
        }
        last_order += batch.len();
        all.extend(batch);
        if last_order > 200 {
            break;
        }
    }
    all
}

fn search_users(query: &str, lang: &str, page: u64) -> Paged<CatalogItem> {
    let target = format!(
        "{BASE_URL}/search/users?s_mode=s_usr&nick={}&i=1&comment=&p={page}",
        url::query_escape(query)
    );
    let body = fetch_document_or_fixture(&target, USER_SEARCH_FIXTURE);
    let next_data = html::text_between(&body, "<script id=\"__NEXT_DATA__\"", "</script>")
        .or_else(|| html::text_between(&body, "<script id='__NEXT_DATA__'", "</script>"))
        .unwrap_or_default();
    let value = serde_json::from_str::<Value>(&next_data).unwrap_or(Value::Null);
    let props = &value["props"]["pageProps"];
    let users = &props["userData"]["users"];
    let entries = props
        .get("userIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|id| {
            let id_string = id
                .as_u64()
                .map(|value| value.to_string())
                .or_else(|| id.as_str().map(ToString::to_string))?;
            let user = users.get(&id_string).unwrap_or(&Value::Null);
            let title = user
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("User {id_string}"));
            Some(catalog_item(
                &format!("/users/{id_string}"),
                &title,
                user.get("imageBig").and_then(Value::as_str),
                lang,
            ))
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn chapter_from_illust(illust: &Value) -> MangaChapter {
    let id = illust
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    MangaChapter {
        key: format!("/artworks/{id}"),
        title: illust
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| Some("Untitled".to_string())),
        date_uploaded: illust
            .get("upload_timestamp")
            .and_then(Value::as_i64)
            .map(|timestamp| timestamp * 1000),
        url: Some(format!("{BASE_URL}/artworks/{id}")),
        ..MangaChapter::default()
    }
}

fn image_url(urls: &Value, quality: &str) -> Option<String> {
    let order = ["thumb_mini", "small", "regular", "original"];
    let start = order
        .iter()
        .position(|name| *name == quality)
        .unwrap_or(order.len() - 1);
    order[start..]
        .iter()
        .filter_map(|name| urls.get(*name).and_then(Value::as_str))
        .next()
        .map(ToString::to_string)
}

fn text_filter(filters: &Value, id: &str) -> Option<String> {
    filters
        .get(id)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn apply_post_filters(items: &mut Vec<CatalogItem>, filters: &Value) {
    if let Some(tags) = text_filter(filters, "tags").filter(|value| !value.is_empty()) {
        let parts = tags
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        items.retain(|item| {
            let tags = item
                .tags
                .iter()
                .map(|tag| tag.to_lowercase())
                .collect::<Vec<_>>();
            parts
                .iter()
                .any(|part| tags.iter().any(|tag| tag.contains(part)))
        });
    }
    if let Some(users) = text_filter(filters, "users").filter(|value| !value.is_empty()) {
        let users = users.to_lowercase();
        items.retain(|item| {
            item.authors
                .iter()
                .any(|author| author.to_lowercase().contains(&users))
        });
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn is_digits(input: &str) -> bool {
    !input.is_empty() && input.bytes().all(|byte| byte.is_ascii_digit())
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"error":false,"body":{"illusts":[{"id":"123","title":"Sample manga","url":"https://i.pximg.net/c/250x250/sample.jpg","type":"manga","is_ad_container":0,"x_restrict":"0","tags":["manga"],"author_details":{"user_id":"45","user_name":"Artist"}}]}}"#;
const LATEST_FIXTURE: &str = SEARCH_FIXTURE;
const RANKING_FIXTURE: &str = r#"{"error":false,"body":{"ranking":[{"illustId":"123","rank":1}]}}"#;
const MANY_DETAILS_FIXTURE: &str = r#"{"error":false,"body":{"illust_details":[{"id":"123","title":"Sample manga","url":"https://i.pximg.net/c/250x250/sample.jpg","type":"manga","is_ad_container":0,"series":{"id":"777","title":"Sample series","userId":"45","coverImage":"https://i.pximg.net/c/250x250/series.jpg"},"tags":["manga"],"author_details":{"user_id":"45","user_name":"Artist"}}]}}"#;
const ILLUST_FIXTURE: &str = r#"{"error":false,"body":{"illust_details":{"id":"123","title":"Sample manga","comment":"<p>About</p>","url":"https://i.pximg.net/c/250x250/sample.jpg","type":"manga","upload_timestamp":1700000000,"x_restrict":"0","tags":["manga"],"author_details":{"user_id":"45","user_name":"Artist"}}}}"#;
const SERIES_FIXTURE: &str = r#"{"error":false,"body":{"series":{"id":"777","title":"Sample series","caption":"Series caption","userId":"45","coverImage":"https://i.pximg.net/c/250x250/series.jpg"}}}"#;
const SERIES_CONTENT_FIXTURE: &str = r#"{"error":false,"body":{"series_contents":[{"id":"123","title":"Chapter 1","url":"https://i.pximg.net/c/250x250/sample.jpg","type":"manga","upload_timestamp":1700000000,"tags":["manga"],"author_details":{"user_id":"45","user_name":"Artist"}}]}}"#;
const USER_FIXTURE: &str = r#"{"error":false,"body":{"userId":"45","name":"Artist","imageBig":"https://i.pximg.net/user.jpg","comment":"Artist profile"}}"#;
const USER_ILLUSTS_FIXTURE: &str = SEARCH_FIXTURE;
const PAGES_FIXTURE: &str = r#"{"error":false,"body":[{"urls":{"thumb_mini":"https://i.pximg.net/t.jpg","small":"https://i.pximg.net/s.jpg","regular":"https://i.pximg.net/r.jpg","original":"https://i.pximg.net/o.jpg"}}]}"#;
const USER_SEARCH_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"userIds":[45],"userData":{"users":{"45":{"userId":"45","name":"Artist","imageBig":"https://i.pximg.net/user.jpg"}}}}}}</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_targets() {
        assert_eq!(
            PixivTarget::parse("https://www.pixiv.net/en/artworks/123"),
            Some(PixivTarget::Illustration("123".into()))
        );
        assert_eq!(
            PixivTarget::parse("https://www.pixiv.net/user/45/series/777"),
            Some(PixivTarget::Series {
                series_id: "777".into(),
                user_id: Some("45".into())
            })
        );
        assert_eq!(
            PixivTarget::parse("user:45"),
            Some(PixivTarget::User("45".into()))
        );
    }

    #[test]
    fn parses_details_chapters_and_pages() {
        let item = SOURCE
            .details(json!({"sourceId":"pixiv-en","manga":"/artworks/123"}))
            .unwrap();
        assert_eq!(item.title, "Sample manga");
        assert_eq!(item.authors, vec!["Artist"]);

        let chapters = SOURCE
            .chapters(json!({"sourceId":"pixiv-en","manga":"/artworks/123"}))
            .unwrap();
        assert_eq!(chapters[0].key, "/artworks/123");

        let pages = SOURCE
            .pages(json!({"sourceId":"pixiv-en","chapter":"/artworks/123","preferences":{"imageQuality":"regular"}}))
            .unwrap();
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn parses_user_search_next_data() {
        let page = search_users("Artist", "en", 1);
        assert_eq!(page.entries[0].key, "/users/45");
    }
}
