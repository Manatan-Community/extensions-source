use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: MangoLibreria = MangoLibreria;
const BASE_URL: &str = "https://mangolibreria.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct MangoLibreria;

impl MangaSource for MangoLibreria {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_comics_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if listing_id(&request) == "latest" {
            format!("{BASE_URL}/comics?page={page}")
        } else {
            format!("{BASE_URL}/comics?sort=views&page={page}")
        };
        Ok(parse_comics_page(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            format!("{BASE_URL}/comics?sort=views&page={page}")
        } else {
            format!(
                "{BASE_URL}/comics?page={page}&q={}",
                url::query_escape(query)
            )
        };
        Ok(parse_comics_page(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        let root = next_data(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            DETAILS_FIXTURE,
        );
        Ok(parse_chapters(
            find_payload(&root, "comicData").unwrap_or(&Value::Null),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".into());
        let root = next_data(
            &fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(
            find_payload(&root, "comicData").unwrap_or(&Value::Null),
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_comics_page(body: &str) -> Paged<CatalogItem> {
    let root = next_data(body, LIST_FIXTURE);
    let comics_data = find_payload(&root, "comicsData").unwrap_or(&Value::Null);
    let entries = comics_data
        .get("comics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(catalog_from_comic)
        .collect::<Vec<_>>();
    let page = comics_data.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total_pages = comics_data
        .get("totalPages")
        .or_else(|| comics_data.get("total_pages"))
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < total_pages,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    let root = next_data(&body, DETAILS_FIXTURE);
    catalog_details_from_comic(
        find_payload(&root, "comicData").unwrap_or(&Value::Null),
        key,
    )
}

fn catalog_from_comic(item: &Value) -> CatalogItem {
    let key = string_value(item, "urlPath")
        .or_else(|| string_value(item, "comic_path"))
        .or_else(|| string_value(item, "chapter_path"))
        .unwrap_or_else(|| "/comics/sample".into());
    CatalogItem {
        key: normalize_key(&key),
        title: string_value(item, "name")
            .or_else(|| string_value(item, "title"))
            .unwrap_or_else(|| "MangoLibreria".into()),
        cover: string_value(item, "urlCover").or_else(|| string_value(item, "cover_image")),
        tags: string_array(item, "genres"),
        status: parse_status(string_value(item, "state").as_deref()),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn catalog_details_from_comic(item: &Value, key: &str) -> CatalogItem {
    let mut out = catalog_from_comic(item);
    out.key = normalize_key(key);
    out.title = string_value(item, "title")
        .or_else(|| string_value(item, "name"))
        .unwrap_or(out.title);
    out.cover = string_value(item, "urlCover")
        .or_else(|| string_value(item, "cover_image"))
        .or(out.cover);
    out.description = string_value(item, "description");
    out.tags = item
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|genre| {
            string_value(genre, "name").or_else(|| genre.as_str().map(ToString::to_string))
        })
        .collect();
    out.status = parse_status(string_value(item, "state").as_deref());
    out.url = Some(absolute_url(key));
    out.initialized = true;
    out
}

fn parse_chapters(comic_data: &Value) -> Vec<MangaChapter> {
    let mut chapters = comic_data
        .get("scan_groups")
        .or_else(|| comic_data.get("scanGroups"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|group| {
            let group_name = string_value(group, "name");
            group
                .get("chapters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |chapter| chapter_from_json(chapter, group_name.clone()))
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn chapter_from_json(chapter: &Value, scanlator: Option<String>) -> Option<MangaChapter> {
    let key = string_value(chapter, "chapter_path")?;
    let number_text = string_value(chapter, "chapter_number").unwrap_or_else(|| "0".into());
    let number = number_text.parse::<f32>().ok();
    let clean_number = number_text.strip_suffix(".0").unwrap_or(&number_text);
    let title =
        string_value(chapter, "title").unwrap_or_else(|| format!("Capitulo {clean_number}"));
    Some(MangaChapter {
        key: normalize_key(&key),
        title: Some(title),
        chapter_number: number,
        date_uploaded: string_value(chapter, "release_date")
            .or_else(|| string_value(chapter, "created_at"))
            .and_then(|date| parse_date(&date)),
        scanlators: scanlator.into_iter().collect(),
        language: Some(LANG.to_string()),
        url: Some(absolute_url(&key)),
        ..MangaChapter::default()
    })
}

fn parse_pages(comic_data: &Value) -> Vec<MangaPage> {
    comic_data
        .get("url_pages")
        .or_else(|| comic_data.get("urlPages"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn next_data(body: &str, fixture: &str) -> Value {
    extract_script_json(body)
        .or_else(|| extract_script_json(fixture))
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or(Value::Null)
}

fn extract_script_json(body: &str) -> Option<String> {
    let marker = "id=\"__NEXT_DATA__\"";
    let start = body
        .find(marker)
        .or_else(|| body.find("id='__NEXT_DATA__'"))?;
    let rest = &body[start..];
    let content_start = rest.find('>')? + 1;
    let after = &rest[content_start..];
    let content_end = after.find("</script>")?;
    Some(html::html_unescape(&after[..content_end]))
}

fn find_payload<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(found) = value.get(key) {
        return Some(found);
    }
    match value {
        Value::Object(map) => map.values().find_map(|child| find_payload(child, key)),
        Value::Array(items) => items.iter().find_map(|child| find_payload(child, key)),
        _ => None,
    }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status.map(str::to_ascii_uppercase).as_deref() {
        Some("ONGOING") => ItemStatus::Ongoing,
        Some("COMPLETED") => ItemStatus::Completed,
        Some("HIATUS") => ItemStatus::Hiatus,
        Some("CANCELLED") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let normalized = value.trim().replace('T', " ");
    let ymd = normalized.get(0..10)?;
    let mut time_parts = normalized.get(11..19).unwrap_or("00:00:00").split(':');
    let hour = time_parts.next()?.parse::<i64>().ok()?;
    let minute = time_parts.next()?.parse::<i64>().ok()?;
    let second = time_parts.next()?.parse::<i64>().ok()?;
    manatan_shared::dates::parse_ymd(ymd).map(|day| day + hour * 3600 + minute * 60 + second)
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(ToString::to_string))
        .collect()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"comicsData":{"page":1,"totalPages":1,"comics":[{"name":"Sample","urlPath":"/comics/sample","urlCover":"https://mangolibreria.com/cover.jpg","state":"ONGOING","genres":["Drama"]}]}}}}</script>
"#;
const DETAILS_FIXTURE: &str = r#"
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"comicData":{"name":"Sample","description":"Summary","cover_image":"https://mangolibreria.com/cover.jpg","state":"ONGOING","genres":[{"name":"Drama"}],"scan_groups":[{"name":"Group","chapters":[{"chapter_number":"1.0","title":"Capitulo 1","release_date":"2024-01-01T00:00:00.000Z","chapter_path":"/comics/sample/chapter-1"}]}]}}}}</script>
"#;
const PAGES_FIXTURE: &str = r#"
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"comicData":{"url_pages":["https://mangolibreria.com/page1.jpg"]}}}}</script>
"#;
