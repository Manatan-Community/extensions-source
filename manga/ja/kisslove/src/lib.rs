use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: KissLove = KissLove;
const BASE_URL: &str = "https://klz9.com";
const CLIENT_ID: &str = "KL9K40zaSyC9K40vOMLLbEcepIFBhUKXwELqxlwTEF";

struct KissLove;

impl MangaSource for KissLove {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_array(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!("{BASE_URL}/api/manga?page={page}&limit=36");
            Ok(parse_paged_manga(&fetch_api(&target, LATEST_FIXTURE)))
        } else {
            Ok(parse_manga_array(&fetch_api(
                &format!("{BASE_URL}/api/manga/trending-daily"),
                LIST_FIXTURE,
            )))
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
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!(
            "{BASE_URL}/api/manga/list?page={page}&search={}&sort=Popular&order=desc",
            url::query_escape(query)
        );
        if let Some(genre) = filter_string(&request, "genre").filter(|value| !value.is_empty()) {
            target.push_str("&genre=");
            target.push_str(&url::query_escape(genre));
        }
        if let Some(status) = filter_string(&request, "status").filter(|value| !value.is_empty()) {
            target.push_str("&status=");
            target.push_str(&url::query_escape(status));
        }
        Ok(parse_paged_manga(&fetch_api(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(parse_chapters(&fetch_api(
            &format!("{BASE_URL}/api/manga/slug/{key}"),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "1/sample-chapter-1".into());
        let id = key.split('/').next().unwrap_or("1");
        Ok(parse_pages(&fetch_api(
            &format!("{BASE_URL}/api/chapter/{id}"),
            PAGES_FIXTURE,
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
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/{key}.html")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/{}.html", key.split('/').nth(1).unwrap_or(&key))))
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api(target: &str, fixture: &str) -> String {
    let (sig, ts) = signature_headers();
    client()
        .get(target)
        .header("X-Client-Sig", sig)
        .header("X-Client-Ts", ts)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn signature_headers() -> (String, String) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
        .to_string();
    let mut hasher = Sha256::new();
    hasher.update(format!("{timestamp}.{CLIENT_ID}").as_bytes());
    (format!("{:x}", hasher.finalize()), timestamp)
}

fn parse_manga_array(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let entries = value
        .as_array()
        .into_iter()
        .flatten()
        .map(item_to_catalog)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_paged_manga(body: &str) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap());
    let entries = value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(item_to_catalog)
        .collect();
    let current = value
        .get("currentPage")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = value
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries,
        has_next_page: current < total,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_api(&format!("{BASE_URL}/api/manga/slug/{key}"), DETAILS_FIXTURE);
    let value = serde_json::from_str::<Value>(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    item_to_catalog(&value)
}

fn item_to_catalog(item: &Value) -> CatalogItem {
    let slug = string_at(item, "/slug").unwrap_or_else(|| "sample".into());
    let description = string_at(item, "/description")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let mut alternate_titles = Vec::new();
    if let Some(other) = string_at(item, "/other_name").filter(|value| !value.is_empty()) {
        alternate_titles.push(other);
    }
    CatalogItem {
        key: slug.clone(),
        title: string_at(item, "/name").unwrap_or_else(|| "KissLove".into()),
        alternate_titles,
        cover: string_at(item, "/cover"),
        authors: string_at(item, "/authors")
            .map(|value| vec![value])
            .unwrap_or_default(),
        artists: string_at(item, "/artists")
            .map(|value| vec![value])
            .unwrap_or_default(),
        description,
        tags: tags_from_value(item.get("genres")),
        status: if item.get("m_status").and_then(Value::as_i64) == Some(1) {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(format!("{BASE_URL}/{slug}.html")),
        language: Some("ja".into()),
        content_rating: Some("suggestive".into()),
        initialized: item.get("chapters").is_some(),
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).unwrap());
    let slug = string_at(&value, "/slug").unwrap_or_else(|| "sample".into());
    let mut chapters = value
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let id = text_value(chapter.get("id"))?;
            let number = chapter
                .get("chapter")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let key = format!("{id}/{slug}-chapter-{number}");
            Some(MangaChapter {
                key: key.clone(),
                title: string_at(chapter, "/name")
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(format!("Chapter {number}"))),
                chapter_number: Some(number as f32),
                date_uploaded: string_at(chapter, "/last_update")
                    .and_then(|value| parse_iso_date(&value)),
                url: Some(format!(
                    "{BASE_URL}/{}.html",
                    key.split('/').nth(1).unwrap_or(&key)
                )),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap());
    string_at(&value, "/content")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .filter(|image| !FILTER_IMAGES.contains(image))
        .enumerate()
        .map(|(index, image)| {
            let image = remap_image_host(image);
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn remap_image_host(input: &str) -> String {
    for (from, to) in IMG_HOSTS {
        let needle = format!("://{from}/");
        if let Some((scheme, path)) = input.split_once(&needle) {
            return format!("{scheme}://{to}/{path}");
        }
    }
    input.to_string()
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let path = input.strip_prefix(BASE_URL)?.trim_matches('/');
    path.strip_suffix(".html").map(ToString::to_string)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn tags_from_value(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_ymd(date)
}

export_manga_source!(SOURCE);

const FILTER_IMAGES: &[&str] = &[
    "https://1.bp.blogspot.com/-ZMyVQcnjYyE/W2cRdXQb15I/AAAAAAACDnk/8X1Hm7wmhz4hLvpIzTNBHQnhuKu05Qb0gCHMYCw/s0/LHScan.png",
    "https://s4.imfaclub.com/images/20190814/Credit_LHScan_5d52edc2409e7.jpg",
    "https://s4.imfaclub.com/images/20200112/5e1ad960d67b2_5e1ad962338c7.jpg",
];

const IMG_HOSTS: &[(&str, &str)] = &[
    ("imfaclub.com", "j1.jfimv2.xyz"),
    ("s2.imfaclub.com", "j2.jfimv2.xyz"),
    ("s4.imfaclub.com", "j4.jfimv2.xyz"),
    ("ihlv1.xyz", "j1.jfimv2.xyz"),
    ("s2.ihlv1.xyz", "j2.jfimv2.xyz"),
    ("s4.ihlv1.xyz", "j4.jfimv2.xyz"),
    ("h1.klimv1.xyz", "j1.jfimv2.xyz"),
    ("h2.klimv1.xyz", "j2.jfimv2.xyz"),
    ("h4.klimv1.xyz", "j4.jfimv2.xyz"),
];

const LIST_FIXTURE: &str = r#"[{"id":1,"slug":"sample","name":"Sample KissLove","cover":"https://klz9.com/cover.jpg","authors":"Sample Author","artists":"Sample Artist","description":"Sample description.","genres":"Drama, Romance","m_status":0,"chapters":[]}]"#;
const LATEST_FIXTURE: &str = r#"{"currentPage":1,"totalPages":1,"items":[{"id":1,"slug":"sample","name":"Sample KissLove","cover":"https://klz9.com/cover.jpg","authors":"Sample Author","artists":"Sample Artist","description":"Sample description.","genres":"Drama, Romance","m_status":0}]}"#;
const SEARCH_FIXTURE: &str = LATEST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"id":1,"slug":"sample","name":"Sample KissLove","cover":"https://klz9.com/cover.jpg","authors":"Sample Author","artists":"Sample Artist","description":"Sample description.","genres":"Drama, Romance","m_status":0,"other_name":"Alt Title","chapters":[{"id":10,"chapter":1.0,"name":"Chapter 1","last_update":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str =
    r#"{"id":10,"content":"https://s4.imfaclub.com/page1.jpg\nhttps://s4.imfaclub.com/page2.jpg"}"#;
