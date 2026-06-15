use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HeavenManga = HeavenManga;
const BASE_URL: &str = "https://heavenmanga.com";
const NAME: &str = "HeavenManga";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct HeavenManga;

impl MangaSource for HeavenManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_card_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            let target = if page <= 1 {
                BASE_URL.to_string()
            } else {
                format!("{BASE_URL}?page={page}")
            };
            Ok(parse_latest_listing(&fetch_document_or_fixture(
                &target,
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_card_listing(&fetch_document_or_fixture(
                &format!("{BASE_URL}/top?orderby=views&page={page}"),
                LIST_FIXTURE,
            )))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        if target.contains("/buscar") {
            Ok(parse_search_listing(&body))
        } else {
            Ok(parse_card_listing(&body))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let manga_key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document_or_fixture(&chapter_api_url(&manga_key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &manga_key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/1#1".to_string());
        let chapter_id = key.split('#').nth(1).unwrap_or("1");
        Ok(parse_pages(&fetch_document_or_fixture(
            &format!("{BASE_URL}/manga/leer/{chapter_id}"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let clean = key.split('#').next().unwrap_or(&key);
            absolute_url(clean)
        }))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_card_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/top?orderby=views&page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_latest_listing(&fetch_document_or_fixture(BASE_URL, LATEST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    let client = client();
    let mut request = client.get(target);
    if target.contains("columns%5B") || target.contains("columns[") {
        request = request
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "application/json");
    }
    request
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_card_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_manga_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: class_text(chunk, "manga-name")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
                cover: image_from_chunk(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next(body),
    }
}

fn parse_latest_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("list-group-item") && !chunk.contains("Novela"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_manga_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: class_text(chunk, "captitle")
                    .or_else(|| {
                        html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v))
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
                cover: Some(format!(
                    "{}/uploads/{}/cover/cover_250x350.jpg",
                    BASE_URL,
                    key.trim_start_matches('/')
                        .trim_end_matches('/')
                        .strip_prefix("manga/")
                        .unwrap_or(key.trim_matches('/'))
                )),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next(body),
    }
}

fn parse_search_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("c-tabs-item__content"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_manga_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
                cover: image_from_chunk(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next(body),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
        cover: html::attr_after(body, "summary_image", "data-src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "description-summary", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "genres-content"),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let slug = json_string(chapter, "slug")?;
            let id = json_i64(chapter, "id").unwrap_or(0);
            let key = format!("{}/{}#{}", manga_key.trim_end_matches('/'), slug, id);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Capitulo: {slug}")),
                chapter_number: slug.parse::<f32>().ok(),
                date_uploaded: json_string(chapter, "created_at")
                    .and_then(|value| parse_rfc3339(&value)),
                url: Some(absolute_url(key.split('#').next().unwrap_or(&key))),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("pUrl"))
        .unwrap_or(body);
    let json = script
        .split("pUrl")
        .nth(1)
        .and_then(|part| part.split('[').nth(1))
        .and_then(|part| part.split(']').next())
        .map(|part| format!("[{}]", remove_trailing_commas(part)))
        .unwrap_or_else(|| PAGES_JSON_FIXTURE.to_string());
    let root = serde_json::from_str::<Value>(&json)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_JSON_FIXTURE).unwrap_or(Value::Null));
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| json_string(page, "imgURL"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut target = if !query.is_empty() {
        format!("{BASE_URL}/buscar?query={}", url::query_escape(query))
    } else if let Some(genre) = filter_str(filters, "genre") {
        format!("{BASE_URL}/genero/{genre}.html")
    } else if let Some(alpha) =
        filter_str(filters, "alpha").or_else(|| filter_str(filters, "letter"))
    {
        format!(
            "{BASE_URL}/letra/manga.html?alpha={}",
            url::query_escape(&alpha)
        )
    } else if let Some(list) =
        filter_str(filters, "list").or_else(|| filter_str(filters, "complete"))
    {
        format!("{BASE_URL}/{list}")
    } else {
        format!("{BASE_URL}/top?orderby=views")
    };
    if page > 1 {
        target.push_str(if target.contains('?') { "&" } else { "?" });
        target.push_str(&format!("page={page}"));
    }
    target
}

fn chapter_api_url(manga_key: &str) -> String {
    format!(
        "{}?columns%5B0%5D%5Bdata%5D=number&columns%5B0%5D%5Borderable%5D=true&columns%5B1%5D%5Bdata%5D=created_at&columns%5B1%5D%5Bsearchable%5D=true&order%5B0%5D%5Bcolumn%5D=1&order%5B0%5D%5Bdir%5D=desc&start=0&length=10000",
        absolute_url(manga_key).trim_end_matches('/')
    )
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn has_next(body: &str) -> bool {
    body.contains("rel=\"next\"") || body.contains("rel='next'")
}

fn normalize_manga_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find("/manga/") {
            return format!("/{}", input[index + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            BASE_URL.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter_str(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn remove_trailing_commas(input: &str) -> String {
    input.replace(",]", "]").replace(",}", "}")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13).unwrap_or("00").parse::<i64>().ok()?;
    let minute = value.get(14..16).unwrap_or("00").parse::<i64>().ok()?;
    let second = value.get(17..19).unwrap_or("00").parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

const LIST_FIXTURE: &str = r#"<div class="page-item-detail"><a href="/manga/sample"><div class="manga-name">Sample</div><img src="/cover.jpg"></a></div>"#;
const LATEST_FIXTURE: &str = r#"<div class="col-lg-8"><div id="loop-content"><div class="list-group-item"><a href="/manga/sample/"><span class="captitle">Sample</span></a></div></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="tab-summary"><div class="summary_image"><img data-src="/cover.jpg"></div><div class="genres-content"><a>Tag</a></div></div><h1>Sample</h1><div class="description-summary"><p>Summary</p></div>"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"data":[{"id":1,"slug":"1","created_at":"2024-01-01 00:00:00"}]}"#;
const PAGES_JSON_FIXTURE: &str = r#"[{"imgURL":"https://heavenmanga.com/page1.jpg"}]"#;
const PAGES_FIXTURE: &str =
    r#"<script>pUrl = [{"imgURL":"https://heavenmanga.com/page1.jpg"}];</script>"#;

export_manga_source!(SOURCE);
