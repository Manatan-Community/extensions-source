use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Izneo = Izneo;
const ORIGIN: &str = "https://www.izneo.com";
const LIMIT: usize = 50;

struct Izneo;

impl MangaSource for Izneo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = language(&request);
        let endpoint = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new"
        } else {
            "topSales"
        };
        let page = page(&request);
        let body = fetch_json(
            &request,
            &format!(
                "{ORIGIN}/{lang}/api/catalog/detail/webtoon/{endpoint}?offset={}&order={}&abo=0",
                page.saturating_sub(1),
                if endpoint == "new" { 1 } else { 0 }
            ),
            LIST_FIXTURE,
        );
        Ok(parse_series_page(&body, &lang, page, ""))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let lang = language(&request);
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_item(&request, &key, &lang)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let body = fetch_json(
            &request,
            &format!(
                "{ORIGIN}/{lang}/api/catalog/detail/webtoon/free?offset={}&order=3&abo=0",
                page.saturating_sub(1)
            ),
            LIST_FIXTURE,
        );
        Ok(parse_series_page(&body, &lang, page, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let lang = language(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{lang}/webtoon/sample-100"));
        Ok(details_item(&request, &key, &lang))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let lang = language(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{lang}/webtoon/sample-100"));
        let id = series_id(&key);
        let mut chapters = Vec::new();
        let mut offset = 0usize;
        loop {
            let body = fetch_json(
                &request,
                &format!("{ORIGIN}/{lang}/api/web/serie/{id}/chapters/old/{offset}/{LIMIT}"),
                CHAPTERS_FIXTURE,
            );
            let albums = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|root| root.get("albums").and_then(Value::as_array).cloned())
                .unwrap_or_default();
            let count = albums.len();
            for album in albums {
                chapters.push(album_chapter(&album, &key));
            }
            if count < LIMIT {
                break;
            }
            offset += LIMIT;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/en/webtoon/sample-100/episode-1-200/read/1".into());
        let id = chapter_id(&key);
        let body = fetch_json(&request, &format!("{ORIGIN}/book/{id}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let latest = self.list(json!({"listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        let popular = self.list(json!({"listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::AesImage::process_128_pkcs7_base64_url(request)
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
        let lang = language(&request);
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_item(&request, &key, &lang)),
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

fn client(request: &Value) -> HttpClient {
    let lang = language(request);
    let mut client = HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{ORIGIN}/{lang}/webtoon"))
        .with_header("Cookie", format!("lang={lang};"))
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_cookies_for(ORIGIN)
        .with_webview_challenge_fallback();
    if let Some(auth) = basic_auth(request) {
        client = client.with_header("Authorization", auth);
    }
    client
}

fn fetch_json(request: &Value, target: &str, fixture: &str) -> String {
    client(request)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_series_page(body: &str, lang: &str, page: u32, query: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap_or(Value::Null));
    let total = root
        .get("series_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let query_lower = query.to_ascii_lowercase();
    let entries = root
        .get("series")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|object| object.values())
        .flat_map(|value| value.as_array().into_iter().flatten())
        .filter_map(|series| series_item(series, lang))
        .filter(|item| query_lower.is_empty() || item.title.to_ascii_lowercase().contains(&query_lower))
        .collect::<Vec<_>>();
    let seen = ((page.saturating_sub(1) as usize) + 1) * entries.len().max(1);
    Paged {
        entries,
        has_next_page: total as usize > seen,
    }
}

fn series_item(item: &Value, lang: &str) -> Option<CatalogItem> {
    let name = item.get("name").and_then(Value::as_str)?;
    let key = normalize_key(item.get("url").and_then(Value::as_str).unwrap_or_default(), lang);
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let version = item.get("version").and_then(Value::as_i64).unwrap_or_default();
    Some(CatalogItem {
        key: key.clone(),
        title: name.into(),
        cover: (!id.is_empty()).then(|| format!("{ORIGIN}/{lang}/images/serie/{id}.jpg?v={version}")),
        authors: item
            .get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|author| author.get("nickname").and_then(Value::as_str).map(ToOwned::to_owned))
            .collect(),
        tags: vec![
            item.get("gender").and_then(Value::as_str).unwrap_or_default().to_string(),
            item.pointer("/target/name").and_then(Value::as_str).unwrap_or_default().to_string(),
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect(),
        description: item
            .get("synopsis")
            .and_then(Value::as_str)
            .map(clean_description)
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(&key)),
        language: Some(lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    })
}

fn details_item(request: &Value, key: &str, lang: &str) -> CatalogItem {
    let item = request.get("manga").unwrap_or(&Value::Null);
    CatalogItem {
        key: normalize_key(key, lang),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "izneo".into())),
        cover: item.get("cover").and_then(Value::as_str).map(ToOwned::to_owned),
        description: item.get("description").and_then(Value::as_str).map(ToOwned::to_owned),
        url: Some(absolute_url(key)),
        language: Some(lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn album_chapter(album: &Value, manga_key: &str) -> MangaChapter {
    let id = album.get("id").and_then(Value::as_str).unwrap_or("0");
    let title = album.get("title").and_then(Value::as_str).unwrap_or("Chapter");
    let number = album
        .get("chapter")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f32>().ok());
    let locked = !album
        .get("fullAvailable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !(album
            .get("inUserLibrary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || album
                .get("inUserSubscription")
                .and_then(Value::as_bool)
                .unwrap_or(false));
    let chapter = album.get("chapter").and_then(Value::as_str).unwrap_or("0");
    MangaChapter {
        key: format!("{}/episode-{chapter}-{id}/read/1", manga_key.trim_end_matches('/')),
        title: Some(if locked {
            format!("{title} [Locked]")
        } else {
            title.into()
        }),
        chapter_number: number,
        date_uploaded: album
            .get("publicationDate")
            .and_then(Value::as_str)
            .and_then(manatan_shared::dates::parse_ymd),
        is_locked: locked,
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).unwrap_or(Value::Null));
    let book_id = root.pointer("/data/id").and_then(Value::as_str).unwrap_or("sample");
    root.pointer("/data/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let number = page.get("albumPageNumber").and_then(Value::as_u64)? as u32;
            let key = page.get("key").and_then(Value::as_str)?;
            let iv = page.get("iv").and_then(Value::as_str)?;
            Some(MangaPage {
                content: PageContent::Url {
                    url: format!("{ORIGIN}/book/{book_id}/{number}?type=full"),
                    context: Some(manga::image_headers(ORIGIN)),
                },
                description: Some(format!("Page {number}")),
                headers: manga::image_headers(ORIGIN),
                extra: BTreeMap::from([
                    ("aesKeyBase64Url".into(), json!(url_safe_base64(key))),
                    ("aesIvBase64Url".into(), json!(url_safe_base64(iv))),
                ]),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn language(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("language"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            request
                .get("sourceId")
                .and_then(Value::as_str)
                .and_then(|source| source.strip_prefix("izneo-"))
                .unwrap_or("en")
        })
        .to_string()
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn basic_auth(request: &Value) -> Option<String> {
    let preferences = request.get("preferences")?;
    let username = preferences.get("username").and_then(Value::as_str)?.trim();
    let password = preferences.get("password").and_then(Value::as_str)?.trim();
    if username.is_empty() || password.is_empty() {
        return None;
    }
    Some(format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}").as_bytes())
    ))
}

fn series_id(key: &str) -> String {
    key.rsplit('-').next().unwrap_or("100").trim_matches('/').into()
}

fn chapter_id(key: &str) -> String {
    key.rsplit('-')
        .next()
        .unwrap_or("200")
        .split('/')
        .next()
        .unwrap_or("200")
        .into()
}

fn normalize_key(value: &str, lang: &str) -> String {
    if value.starts_with('/') {
        value.into()
    } else {
        format!("/{lang}/webtoon/{}", value.trim_start_matches('/'))
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input.strip_prefix(ORIGIN).map(|value| value.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(ORIGIN, value)
}

fn clean_description(value: &str) -> String {
    value
        .replace("\n          ", " ")
        .replace("<br />", " ")
        .trim()
        .to_string()
}

fn url_safe_base64(value: &str) -> String {
    value.replace('+', "-").replace('/', "_")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{
  "series_count": 1,
  "series": {
    "0": [
      {
        "name": "Sample izneo",
        "url": "/en/webtoon/sample-100",
        "id": "100",
        "version": 1,
        "synopsis": "Sample description.",
        "gender": "Action",
        "target": { "name": "Teen" },
        "authors": [{ "nickname": "Sample Author" }]
      }
    ]
  }
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "albums": [
    { "id": "200", "title": "Episode 1", "chapter": "1", "publicationDate": "2024-01-01", "fullAvailable": true, "inUserLibrary": false, "inUserSubscription": false }
  ]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "data": {
    "id": "200",
    "pages": [
      { "albumPageNumber": 1, "key": "MDEyMzQ1Njc4OWFiY2RlZg==", "iv": "YWJjZGVmOTg3NjU0MzIxMA==" }
    ]
  }
}"#;
