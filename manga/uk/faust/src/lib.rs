use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, Viewer, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{dates, manga, url};
use serde_json::{Value, json};

const SOURCE: Faust = Faust;
const BASE_URL: &str = "https://faust-web.com";
const API_URL: &str = "https://faust-web.com/api";

struct Faust;

impl MangaSource for Faust {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "+updated"
        } else {
            "-rating"
        };
        Ok(parse_catalog(&post_json(
            &format!("{API_URL}/titles/search/library"),
            search_body(page(&request), "", sort, &Value::Null),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let key = query
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
                )],
                has_next_page: false,
            });
        }
        let sort = format!(
            "{}{}",
            filter_string(&request, "sortDirection").unwrap_or_else(|| "-".to_string()),
            filter_string(&request, "sortBy").unwrap_or_else(|| "rating".to_string())
        );
        Ok(parse_catalog(&post_json(
            &format!("{API_URL}/titles/search/library"),
            search_body(
                page(&request),
                query,
                &sort,
                request.get("filters").unwrap_or(&Value::Null),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_details(
            &fetch_json(&details_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_chapters(
            &fetch_json(&details_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter-slug/sample".to_string());
        Ok(parse_pages(
            &fetch_json(&chapter_api_url(&key), PAGES_FIXTURE),
            &key,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/manga/{key}")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (chapter_slug, series_slug) = split_once(&key, '/');
            let mut pieces = chapter_slug.split('-');
            let a = pieces.next().unwrap_or("chapter");
            let b = pieces.next().unwrap_or("1");
            let c = pieces.next().unwrap_or("page");
            let d = pieces.next().unwrap_or("1");
            format!("{BASE_URL}/manga/{series_slug}/{a}-{b}/{c}-{d}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = input
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample");
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_json(&details_url(key), DETAILS_FIXTURE),
                    key,
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
        .with_origin(BASE_URL)
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("Content-Type", "application/json")
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

fn post_json(target: &str, body: Value, fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .json(body.to_string())
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_body(page: u64, query: &str, sort: &str, filters: &Value) -> Value {
    let mut body = json!({
        "searchQuery": query,
        "page": page,
        "pageSize": 30,
        "sortBy": sort
    });
    for key in [
        "mangaType",
        "translationStatus",
        "publicationStatus",
        "ageBracket",
        "yearFrom",
        "yearTo",
        "minChapters",
        "maxChapters",
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            body[key] = json!(value);
        }
    }
    body
}

fn details_url(key: &str) -> String {
    format!(
        "{API_URL}/titles/{}",
        url::query_escape(key.trim_matches('/'))
    )
}

fn chapter_api_url(key: &str) -> String {
    let (chapter_slug, series_slug) = split_once(key, '/');
    format!(
        "{API_URL}/chapters/{}?titleSlug={}",
        url::query_escape(chapter_slug),
        url::query_escape(series_slug)
    )
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    let root = parse_json(body);
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = root
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    let entries = root
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let key = string(item, "slug");
            CatalogItem {
                key: key.clone(),
                title: string(item, "name"),
                cover: opt_string(item, "coverImageUrl"),
                url: Some(format!("{BASE_URL}/manga/{key}")),
                language: Some("uk".to_string()),
                content_rating: Some("safe".to_string()),
                viewer: Some(Viewer::RightToLeft),
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: total > page,
    }
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let root = parse_json(body);
    let key = opt_string(&root, "slug").unwrap_or_else(|| fallback_key.to_string());
    let mut description = root
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(english) = root
        .get("englishName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Альтернативні назви: ");
        description.push_str(english);
    }
    if let Some(rating) = root.get("averageRating").and_then(Value::as_f64) {
        description.push_str(&format!("\nРейтинг: {rating:.2}/5"));
    }
    let mut tags = Vec::new();
    if let Some(kind) = root.get("mangaType").and_then(Value::as_str) {
        tags.push(manga_type(kind).to_string());
    }
    tags.extend(name_list(root.get("genres")));
    tags.extend(name_list(root.get("tags")));
    CatalogItem {
        key: key.clone(),
        title: string(&root, "name"),
        cover: opt_string(&root, "coverImageUrl"),
        url: Some(format!("{BASE_URL}/manga/{key}")),
        authors: people(root.get("authors")),
        artists: people(root.get("artists")),
        description: (!description.is_empty()).then_some(description),
        tags,
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        status: match root.get("translationStatus").and_then(Value::as_str) {
            Some("Inactive") => ItemStatus::Cancelled,
            Some("Translated") => ItemStatus::Completed,
            Some("Active") => ItemStatus::Ongoing,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, series_slug: &str) -> Vec<MangaChapter> {
    let root = parse_json(body);
    let slug = opt_string(&root, "slug").unwrap_or_else(|| series_slug.to_string());
    let mut chapters = Vec::new();
    for volume in root
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for chapter in volume
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let chapter_slug = string(chapter, "slug");
            let number = chapter.get("number").and_then(Value::as_f64).unwrap_or(0.0);
            let volume_order = chapter
                .get("volumeOrder")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let name = string(chapter, "name");
            let title = if name.contains("Розділ") {
                format!("Том {} {name}", compact_float(volume_order))
            } else {
                format!(
                    "Том {} Розділ {} {name}",
                    compact_float(volume_order),
                    compact_float(number)
                )
            };
            chapters.push(MangaChapter {
                key: format!("{chapter_slug}/{slug}"),
                title: Some(title.trim().to_string()),
                chapter_number: Some(number as f32),
                date_uploaded: chapter
                    .get("updatedDate")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date),
                scanlators: name_list(chapter.get("translationTeams")),
                language: Some("uk".to_string()),
                url: Some(format!("{BASE_URL}/manga/{slug}")),
                ..MangaChapter::default()
            });
        }
    }
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, key: &str) -> Vec<MangaPage> {
    let referer = format!("{BASE_URL}/manga/{}", split_once(key, '/').1);
    parse_json(body)
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|page| {
            let page_number = page.get("pageNumber").and_then(Value::as_u64).unwrap_or(1);
            let image = string(page, "blobName");
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(&referer)),
                },
                headers: manga::image_headers(&referer),
                description: Some(format!("Page {page_number}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string(value: &Value, key: &str) -> String {
    opt_string(value, key).unwrap_or_default()
}

fn opt_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn name_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn people(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            format!(
                "{} {}",
                item.get("firstName")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                item.get("lastName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
            .trim()
            .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn split_once(value: &str, delimiter: char) -> (&str, &str) {
    value.split_once(delimiter).unwrap_or((value, "sample"))
}

fn manga_type(kind: &str) -> &str {
    match kind {
        "Manga" => "Манґа",
        "Manhwa" => "Манхва",
        "Manhua" => "Маньхва",
        "Oneshot" => "Ваншот",
        "Webcomic" => "Вебкомікс",
        "Doujinshi" => "Доджінші",
        "Extra" => "Екстра",
        "Comics" => "Комікс",
        "Malyopys" => "Мальопис",
        _ => kind,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    value.get(..10).and_then(dates::parse_ymd)
}

fn compact_float(value: f64) -> String {
    if value.fract() == 0.0 {
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

const LIST_FIXTURE: &str = r#"{"page":1,"totalPages":1,"titles":[{"name":"Sample Faust","slug":"sample","coverImageUrl":"https://faust-web.com/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"{"name":"Sample Faust","slug":"sample","coverImageUrl":"https://faust-web.com/cover.jpg","description":"Fixture","artists":[],"authors":[],"mangaType":"Manga","tags":[],"genres":[],"translationStatus":"Active","averageRating":5,"englishName":"","votesCount":1}"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":[{"blobName":"https://faust-web.com/page-1.jpg","pageNumber":1}]}"#;
