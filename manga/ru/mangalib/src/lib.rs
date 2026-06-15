use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MangaLib = MangaLib;
const DEFAULT_BASE_URL: &str = "https://mangalib.me";
const DEFAULT_API: &str = "https://api.cdnlibs.org";
const SITE_ID: &str = "1";

struct MangaLib;

impl MangaSource for MangaLib {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL, "eng"));
        }
        let base = base_url(&request);
        let api = api_domain(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{api}/api/latest-updates?page={page}")
        } else {
            format!("{api}/api/manga?site_id[]={SITE_ID}&page={page}")
        };
        Ok(parse_listing(
            &fetch_text(&request, &target, LIST_FIXTURE),
            &base,
            &title_language(&request),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let api = api_domain(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with("http://")
            || query.starts_with("https://")
            || query.starts_with("slug:")
        {
            let key = normalize_key(&base, query);
            let body = fetch_text(&request, &details_api_url(&api, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(
                    &body,
                    Some(key),
                    &base,
                    &title_language(&request),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(
            &fetch_text(
                &request,
                &search_url(
                    &api,
                    request.get("page").and_then(Value::as_u64).unwrap_or(1),
                    query,
                    request.get("filters").unwrap_or(&Value::Null),
                ),
                LIST_FIXTURE,
            ),
            &base,
            &title_language(&request),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let api = api_domain(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1--sample".into());
        Ok(parse_details(
            &fetch_text(&request, &details_api_url(&api, &key), DETAILS_FIXTURE),
            Some(key),
            &base,
            &title_language(&request),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let api = api_domain(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/1--sample".into());
        Ok(parse_chapters(
            &fetch_text(
                &request,
                &format!("{api}/api/manga{}/chapters", key.trim_end_matches('/')),
                CHAPTERS_FIXTURE,
            ),
            &key,
            show_paid(&request),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let api = api_domain(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/1--sample/chapter?&volume=1&number=1".into());
        let body = fetch_text(&request, &format!("{api}/api/manga{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&request, &body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{base}/ru/manga{}", key.trim_end_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter")
            .map(|key| chapter_web_url(&base, &key, user_id(&request).as_deref())))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(&base) {
            let key = normalize_key(&base, input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_text(
                        &request,
                        &details_api_url(&api_domain(&request), &key),
                        DETAILS_FIXTURE,
                    ),
                    Some(key),
                    &base,
                    &title_language(&request),
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

fn client(request: &Value) -> HttpClient {
    let base = base_url(request);
    let mut client = HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(base.clone())
        .with_header("Site-Id", SITE_ID)
        .with_cookies_for(&base)
        .with_webview_challenge_fallback();
    if let Some(token) = bearer_token(request) {
        client = client.with_header("Authorization", format!("Bearer {token}"));
    }
    client
}

fn fetch_text(request: &Value, target: &str, fixture: &str) -> String {
    client(request)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get("domain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn api_domain(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get("apiDomain"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_API.to_string())
}

fn title_language(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get("titleLanguage"))
        .and_then(Value::as_str)
        .unwrap_or("eng")
        .to_string()
}

fn bearer_token(request: &Value) -> Option<String> {
    request
        .get("preferences")
        .and_then(|p| p.get("bearerToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
}

fn user_id(request: &Value) -> Option<String> {
    request
        .get("preferences")
        .and_then(|p| p.get("userId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
}

fn show_paid(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|p| p.get("showPaidChapters"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn search_url(api: &str, page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![format!("page={page}"), format!("site_id[]={SITE_ID}")];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    if let Some(sort) = filter_id(filters, "sort") {
        params.push(format!("sort_by={sort}"));
    }
    params.push(format!(
        "sort_type={}",
        filter_id(filters, "sortDirection").unwrap_or("desc")
    ));
    for (id, param) in [
        ("types", "types[]"),
        ("scanlateStatus", "scanlate_status[]"),
        ("status", "status[]"),
        ("age", "caution[]"),
        ("genres", "genres[]"),
    ] {
        for value in selected_values(filters.get(id)) {
            params.push(format!("{param}={value}"));
        }
    }
    format!("{api}/api/manga?{}", params.join("&"))
}

fn details_api_url(api: &str, key: &str) -> String {
    let fields = [
        "eng_name",
        "otherNames",
        "summary",
        "rate",
        "genres",
        "tags",
        "teams",
        "authors",
        "publisher",
        "userRating",
        "manga_status_id",
        "status_id",
        "artists",
    ];
    format!(
        "{api}/api/manga{}?{}",
        key.trim_end_matches('/'),
        fields
            .iter()
            .map(|field| format!("fields[]={field}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str, base: &str, lang: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = format!(
                "/{}",
                text(item, "slug_url")
                    .unwrap_or_else(|| "1--sample".into())
                    .trim_start_matches('/')
            );
            CatalogItem {
                key: key.clone(),
                title: selected_title(item, lang),
                cover: item
                    .pointer("/cover/default")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: Some(format!("{base}/ru/manga{}", key.trim_end_matches('/'))),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect::<Vec<_>>();
    let has_next_page = root
        .pointer("/meta/has_next_page")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>, base: &str, lang: &str) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let data = root.get("data").unwrap_or(&root);
    let key = key.unwrap_or_else(|| {
        format!(
            "/{}",
            text(data, "slug_url").unwrap_or_else(|| "1--sample".into())
        )
    });
    let rating = data
        .pointer("/rating/average")
        .and_then(Value::as_f64)
        .map(|v| v as f32);
    let mut description = String::new();
    if let Some(opposite) = opposite_title(data, lang) {
        description.push_str(&opposite);
        description.push('\n');
    }
    if let Some(rating) = rating {
        let votes = data
            .pointer("/rating/votes")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        description.push_str(&format!("Рейтинг: {rating} (голосов: {votes})\n"));
    }
    if let Some(names) = data
        .get("otherNames")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    {
        description.push_str("Альтернативные названия:\n");
        description.push_str(
            &names
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" / "),
        );
        description.push_str("\n\n");
    }
    if let Some(summary) = summary_text(data.get("summary")) {
        description.push_str(&summary);
    }
    let scanlate = data
        .pointer("/scanlateStatus/label")
        .or_else(|| data.pointer("/scanlate_status/label"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let status = data
        .pointer("/status/label")
        .and_then(Value::as_str)
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: selected_title(data, lang),
        alternate_titles: data
            .get("otherNames")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        cover: data
            .pointer("/cover/default")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: names(data.get("authors")),
        artists: names(data.get("artists")),
        tags: [names(data.get("genres")), names(data.get("tags"))].concat(),
        rating,
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        status: parse_status(
            data.get("is_licensed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            scanlate,
            status,
        ),
        url: Some(format!("{base}/ru/manga{}", key.trim_end_matches('/'))),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str, show_paid: bool) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let slug = manga_key.trim_start_matches('/').trim_end_matches('/');
    let mut chapters = root
        .pointer("/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|chapter| {
            let branches = chapter
                .get("branches")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first = branches.first().cloned().unwrap_or(Value::Null);
            let open = first
                .pointer("/restricted_view/is_open")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !show_paid && !open {
                return Vec::new();
            }
            let branch_id = first.get("branch_id").and_then(Value::as_i64);
            let number = text(chapter, "number").unwrap_or_else(|| "1".into());
            let volume = text(chapter, "volume").unwrap_or_else(|| "1".into());
            let name = text(chapter, "name").unwrap_or_default();
            let branch_part = branch_id
                .map(|id| format!("&branch_id={id}"))
                .unwrap_or_default();
            let key = format!("/{slug}/chapter?{branch_part}&volume={volume}&number={number}");
            let mut title = format!("Том {volume}. Глава {number}");
            if !name.is_empty() {
                title.push_str(&format!(" - {name}"));
            }
            if !open {
                title = format!("$$ {title}");
            }
            vec![MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: number.parse().ok(),
                volume_number: volume.parse().ok(),
                date_uploaded: first
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                scanlators: first
                    .get("teams")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|team| text(team, "name"))
                    .collect(),
                url: Some(key),
                language: Some("ru".into()),
                is_locked: !open,
                ..MangaChapter::default()
            }]
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(request: &Value, body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let server = image_server(request);
    let mut pages = root
        .pointer("/data/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let index = page.get("slug").and_then(Value::as_u64).unwrap_or(0);
            let image = text(page, "url")?;
            let image_url = if image.starts_with("http://") || image.starts_with("https://") {
                image
            } else {
                format!("{}{}", server.trim_end_matches('/'), image)
            };
            Some((
                index,
                MangaPage {
                    content: PageContent::Url {
                        url: image_url,
                        context: Some(manga::image_headers(&base_url(request))),
                    },
                    headers: manga::image_headers(&base_url(request)),
                    description: Some(format!("Page {index}")),
                    ..MangaPage::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    pages.sort_by_key(|(index, _)| *index);
    pages.into_iter().map(|(_, page)| page).collect()
}

fn image_server(request: &Value) -> String {
    match request
        .get("preferences")
        .and_then(|p| p.get("imageServer"))
        .and_then(Value::as_str)
        .unwrap_or("compress")
    {
        "main" => "https://img2.imglib.info",
        "secondary" => "https://img3.imglib.info",
        _ => "https://img33.imgslib.link",
    }
    .to_string()
}

fn chapter_web_url(base: &str, key: &str, user_id: Option<&str>) -> String {
    let slug = key
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    let volume = query_part(key, "volume").unwrap_or("1");
    let number = query_part(key, "number").unwrap_or("1");
    let branch = query_part(key, "branch_id")
        .map(|id| format!("&bid={id}"))
        .unwrap_or_default();
    let user = user_id.map(|id| format!("&ui={id}")).unwrap_or_default();
    format!("{base}/ru/{slug}/read/v{volume}/c{number}?{branch}{user}")
}

fn query_part<'a>(value: &'a str, key: &str) -> Option<&'a str> {
    value.split('?').nth(1)?.split('&').find_map(|part| {
        part.split_once('=')
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
    })
}

fn selected_title(value: &Value, lang: &str) -> String {
    match lang {
        "rus" => text(value, "rus_name")
            .or_else(|| text(value, "name"))
            .or_else(|| text(value, "eng_name")),
        _ => text(value, "eng_name")
            .or_else(|| text(value, "name"))
            .or_else(|| text(value, "rus_name")),
    }
    .unwrap_or_else(|| "MangaLib".into())
}

fn opposite_title(value: &Value, lang: &str) -> Option<String> {
    match lang {
        "rus" => text(value, "eng_name"),
        _ => text(value, "rus_name"),
    }
    .filter(|title| !title.is_empty())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.is_empty())
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| text(item, "name").or_else(|| text(item, "label")))
        .collect()
}

fn summary_text(value: Option<&Value>) -> Option<String> {
    fn walk(value: &Value, out: &mut String) {
        match value {
            Value::String(text) => out.push_str(text),
            Value::Array(values) => values.iter().for_each(|item| walk(item, out)),
            Value::Object(map) => {
                if map.get("type").and_then(Value::as_str) == Some("hardBreak") {
                    out.push('\n');
                }
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
                if let Some(content) = map.get("content") {
                    walk(content, out);
                    if map.get("type").and_then(Value::as_str) == Some("paragraph") {
                        out.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(value?, &mut out);
    (!out.trim().is_empty()).then(|| out.trim().to_string())
}

fn parse_status(licensed: bool, scanlate: &str, title: &str) -> ItemStatus {
    if licensed {
        return ItemStatus::Unknown;
    }
    match (scanlate, title) {
        ("Продолжается", _) | ("Выходит", _) => ItemStatus::Ongoing,
        ("Завершён", "Приостановлен") | ("Заморожен", _) | ("Заброшен", _) => {
            ItemStatus::Hiatus
        }
        ("Завершён", "Выпуск прекращён") => ItemStatus::Cancelled,
        ("Завершён", _) => ItemStatus::Completed,
        _ if title == "Онгоинг" || title == "Анонс" => ItemStatus::Ongoing,
        _ if title == "Завершён" => ItemStatus::Completed,
        _ if title == "Приостановлен" => ItemStatus::Hiatus,
        _ if title == "Выпуск прекращён" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(..10)?)
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(option_id)
            .collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value)))
        .filter(|value| !value.is_empty())
}

fn option_id(value: &str) -> Option<String> {
    let id = value
        .trim()
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

fn normalize_key(base: &str, value: &str) -> String {
    let value = value
        .strip_prefix("slug:")
        .map(|slug| format!("/{slug}"))
        .unwrap_or_else(|| value.to_string());
    let mut path = value
        .strip_prefix(base)
        .unwrap_or(&value)
        .split('?')
        .next()
        .unwrap_or(&value)
        .trim_start_matches('/')
        .to_string();
    path = path
        .strip_prefix("ru/manga/")
        .unwrap_or(&path)
        .strip_prefix("manga/")
        .unwrap_or(&path)
        .to_string();
    format!("/{}", path.trim_matches('/'))
}

const LIST_FIXTURE: &str = r#"{"data":[{"name":"Sample","rus_name":"Пример","eng_name":"Sample","slug_url":"1--sample","cover":{"default":"https://img33.imgslib.link/sample.jpg"}}],"meta":{"has_next_page":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"name":"Sample","rus_name":"Пример","eng_name":"Sample","slug_url":"1--sample","cover":{"default":"https://img33.imgslib.link/sample.jpg"},"authors":[],"artists":[],"genres":[],"tags":[],"rating":{"average":8.0,"votes":1},"status":{"label":"Онгоинг"},"scanlateStatus":{"label":"Продолжается"},"is_licensed":false,"otherNames":[],"summary":"Description"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"id":1,"branches_count":1,"branches":[{"branch_id":1,"created_at":"2024-01-01T00:00:00.000000Z","teams":[{"name":"Team"}],"restricted_view":{"is_open":true},"user":{"username":"user"}}],"name":null,"number":"1","volume":"1"}]}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"pages":[{"slug":1,"url":"/manga/sample/1.jpg"},{"slug":2,"url":"/manga/sample/2.jpg"}]}}"#;

export_manga_source!(SOURCE);
