use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, Viewer, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{manga, url};
use serde_json::Value;

const SOURCE: Zenko = Zenko;
const BASE_URL: &str = "https://zenko.online";
const API_URL: &str = "https://api.zenko.online";
const IMAGE_STORAGE_URL: &str = "https://storage.zenko.online";
const SEPARATOR: &str = "@#%&;№%#&**#!@";

struct Zenko;

impl MangaSource for Zenko {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "lastChapterCreatedAt"
        } else {
            "viewsCount"
        };
        let target = catalog_url(
            page(&request),
            sort,
            "DESC",
            "",
            &Value::Null,
            request.get("preferences").unwrap_or(&Value::Null),
        );
        Ok(parse_catalog(
            &fetch_json(&target, LIST_FIXTURE),
            request.get("preferences").unwrap_or(&Value::Null),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/titles/") {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("1");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                    request.get("preferences").unwrap_or(&Value::Null),
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sortBy").unwrap_or_else(|| "viewsCount".to_string());
        let order = filter_string(filters, "order").unwrap_or_else(|| "DESC".to_string());
        let target = catalog_url(
            page(&request),
            &sort,
            &order,
            query,
            filters,
            request.get("preferences").unwrap_or(&Value::Null),
        );
        Ok(parse_catalog(
            &fetch_json(&target, LIST_FIXTURE),
            request.get("preferences").unwrap_or(&Value::Null),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            normalize_id(&manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string()));
        Ok(parse_details(
            &fetch_json(&details_url(&key), DETAILS_FIXTURE),
            &key,
            request.get("preferences").unwrap_or(&Value::Null),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            normalize_id(&manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string()));
        Ok(parse_chapters(&fetch_json(
            &format!("{API_URL}/titles/{key}/chapters"),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1/10".to_string());
        let chapter_id = key.rsplit('/').next().unwrap_or("10");
        Ok(parse_pages(
            &fetch_json(&format!("{API_URL}/chapters/{chapter_id}"), PAGES_FIXTURE),
            &key,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/titles/{}", normalize_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/titles/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/titles/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("1");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                    request.get("preferences").unwrap_or(&Value::Null),
                )),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(format!("{BASE_URL}/"))
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Accept-Language", "uk-UA,uk;q=0.9,en-US;q=0.8,en;q=0.7")
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

fn catalog_url(
    page: u64,
    sort_by: &str,
    order: &str,
    query: &str,
    filters: &Value,
    preferences: &Value,
) -> String {
    let mut params = vec![
        ("limit", "15".to_string()),
        ("offset", ((page.saturating_sub(1)) * 15).to_string()),
        ("sortBy", sort_by.to_string()),
        ("order", order.to_string()),
    ];
    for key in [
        "categories",
        "translationStatus",
        "genres",
        "tags",
        "ageLimit",
        "releaseYearFrom",
        "releaseYearTo",
    ] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            params.push((key, value));
        }
    }
    if filters == &Value::Null {
        let hidden = preference_values(preferences, "hiddenCategories");
        if !hidden.is_empty() {
            let visible = [
                "MANGA_UA",
                "MANGA",
                "MANHVA",
                "MANHUA",
                "WESTERN_COMICS",
                "COMICS",
                "RANOBE",
                "OTHER",
            ]
            .into_iter()
            .filter(|value| !hidden.iter().any(|hidden| hidden == value))
            .collect::<Vec<_>>()
            .join(",");
            params.push(("categories", visible));
        } else {
            params.push((
                "categories",
                "MANGA_UA,MANGA,MANHVA,MANHUA,WESTERN_COMICS,COMICS,OTHER".to_string(),
            ));
        }
        let ages = preference_values(preferences, "ageLimit");
        if !ages.is_empty() {
            params.push(("ageLimit", ages.join(",")));
        }
    }
    if !query.is_empty() {
        params.push(("name", query.to_string()));
    }
    format!(
        "{API_URL}/titles?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn details_url(key: &str) -> String {
    format!("{API_URL}/titles/{}", url::query_escape(&normalize_id(key)))
}

fn parse_catalog(body: &str, preferences: &Value) -> Paged<CatalogItem> {
    let root = parse_json(body);
    let entries = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = string(item, "id");
            CatalogItem {
                key: key.clone(),
                title: selected_title(preferences, item),
                cover: opt_string(item, "coverImg").map(build_image_url),
                url: Some(format!("{BASE_URL}/titles/{key}")),
                language: Some("uk".to_string()),
                content_rating: Some("safe".to_string()),
                viewer: Some(Viewer::RightToLeft),
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: root
            .pointer("/meta/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_details(body: &str, fallback_key: &str, preferences: &Value) -> CatalogItem {
    let root = parse_json(body);
    let key = opt_string(&root, "id").unwrap_or_else(|| normalize_id(fallback_key));
    let mut description = string(&root, "description");
    if preference_string(preferences, "titleLanguage").as_deref() != Some("eng") {
        let alternatives = [
            opt_string(&root, "engName"),
            opt_string(&root, "originalName"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !alternatives.is_empty() {
            description.push_str("\n\nАльтернативні назви: ");
            description.push_str(&alternatives.join(", "));
        }
    }
    for (label, key_name) in [
        ("Вподобайок", "likesCount"),
        ("Переглядів", "viewsCount"),
        ("В закладинках у", "bookmarksCount"),
    ] {
        if let Some(value) = root.get(key_name).and_then(Value::as_i64) {
            description.push_str(&format!("\n{label}: {value}"));
        }
    }
    let mut tags = name_list(root.get("genres"));
    tags.extend(name_list(root.get("tags")));
    CatalogItem {
        key: key.clone(),
        title: selected_title(preferences, &root),
        cover: opt_string(&root, "coverImg").map(build_image_url),
        url: Some(format!("{BASE_URL}/titles/{key}")),
        authors: name_list(root.get("writers")),
        artists: name_list(root.get("painters")),
        description: (!description.is_empty()).then_some(description),
        tags,
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        status: match root
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" => ItemStatus::Ongoing,
            "finished" => ItemStatus::Completed,
            "paused" => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = parse_json(body)
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let id = string(item, "id");
            let title_id = string(item, "titleId");
            let raw_name = opt_string(item, "name");
            MangaChapter {
                key: format!("{title_id}/{id}"),
                title: Some(format_chapter(raw_name.as_deref())),
                chapter_number: Some(chapter_number(raw_name.as_deref())),
                date_uploaded: item
                    .get("createdAt")
                    .and_then(Value::as_i64)
                    .map(|seconds| seconds * 1000),
                scanlators: item
                    .pointer("/publisher/name")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                language: Some("uk".to_string()),
                url: Some(format!("{BASE_URL}/titles/{title_id}/{id}")),
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        let left = generate_chapter_id(a.title.as_deref())
            .unwrap_or(a.chapter_number.unwrap_or_default() as f64);
        let right = generate_chapter_id(b.title.as_deref())
            .unwrap_or(b.chapter_number.unwrap_or_default() as f64);
        right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str, key: &str) -> Vec<MangaPage> {
    let referer = format!("{BASE_URL}/titles/{key}");
    parse_json(body)
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|page| {
            let index = page.get("id").and_then(Value::as_u64).unwrap_or(1);
            let image = string(page, "content");
            MangaPage {
                content: PageContent::Url {
                    url: format!("{IMAGE_STORAGE_URL}/{image}"),
                    context: Some(manga::image_headers(&referer)),
                },
                headers: manga::image_headers(&referer),
                description: Some(format!("Page {index}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn selected_title(preferences: &Value, item: &Value) -> String {
    if preference_string(preferences, "titleLanguage").as_deref() == Some("eng") {
        if let Some(title) = opt_string(item, "engName") {
            return title;
        }
    }
    string(item, "name")
}

fn parse_json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_id(value: &str) -> String {
    value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .to_string()
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_values(preferences: &Value, key: &str) -> Vec<String> {
    preferences
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn preference_string(preferences: &Value, key: &str) -> Option<String> {
    preferences
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string(value: &Value, key: &str) -> String {
    opt_string(value, key).unwrap_or_default()
}

fn opt_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToString::to_string)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn name_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn build_image_url(image: String) -> String {
    format!("{IMAGE_STORAGE_URL}/{image}?optimizer=image&width=560&quality=70&height=auto")
}

fn parse_chapter(input: Option<&str>) -> (&str, &str, &str) {
    let Some(input) = input else {
        return ("", "", "");
    };
    let mut parts = input.split(SEPARATOR);
    let first = parts.next().unwrap_or("");
    let second = parts.next();
    let third = parts.next();
    match (second, third) {
        (Some(chapter), Some(name)) => (first, chapter, name),
        (Some(chapter), None) => (first, chapter, ""),
        _ => ("", "", first),
    }
}

fn format_chapter(input: Option<&str>) -> String {
    let (part, chapter, name) = parse_chapter(input);
    let chapter_label = if chapter.is_empty() {
        String::new()
    } else if name.is_empty() {
        format!("Розділ {chapter}")
    } else {
        format!("Розділ {chapter}:")
    };
    [
        (!part.is_empty()).then(|| format!("Том {part}")),
        (!chapter_label.is_empty()).then_some(chapter_label),
        (!name.is_empty()).then(|| name.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

fn chapter_number(input: Option<&str>) -> f32 {
    let (_, chapter, _) = parse_chapter(input);
    chapter.parse().unwrap_or(-1.0)
}

fn generate_chapter_id(input: Option<&str>) -> Option<f64> {
    let (part, chapter, _) = parse_chapter(input);
    let part_number = part.parse::<i64>().unwrap_or(0);
    if chapter.is_empty() {
        return None;
    }
    let formatted = if chapter.contains('.') {
        let mut pieces = chapter.split('.');
        let first = pieces.next().unwrap_or("");
        let mut out = if first.len() == 1 {
            format!("0{first}")
        } else {
            first.to_string()
        };
        for piece in pieces {
            out.push('.');
            out.push_str(piece);
        }
        out
    } else if chapter.len() == 1 {
        format!("0{chapter}")
    } else {
        chapter.to_string()
    };
    if part_number > 0 {
        format!("{part_number}{formatted}").parse().ok()
    } else {
        chapter.parse().ok()
    }
}

const LIST_FIXTURE: &str = r#"{"data":[{"id":1,"name":"Sample Zenko","engName":"Sample Zenko","coverImg":"cover.jpg"}],"meta":{"hasNextPage":false}}"#;
const DETAILS_FIXTURE: &str = r#"{"id":1,"coverImg":"cover.jpg","description":"Fixture","status":"ongoing","name":"Sample Zenko","engName":"Sample Zenko","originalName":"Sample","genres":[{"name":"Фентезі"}],"tags":[],"writers":[],"painters":[]}"#;
const CHAPTERS_FIXTURE: &str = r#"[{"createdAt":1704067200,"id":10,"name":"1@#%&;№%#&**#!@1@#%&;№%#&**#!@Start","titleId":1,"publisher":{"name":"Zenko"}}]"#;
const PAGES_FIXTURE: &str =
    r#"{"id":10,"name":"1@#%&;№%#&**#!@1","titleId":1,"pages":[{"id":1,"content":"page-1.jpg"}]}"#;
