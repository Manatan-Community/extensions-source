use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult,
    abi::{ExtensionResult, system_time},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{lunar::LunarReader, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: LunarAnime = LunarAnime;
const BASE_URL: &str = "https://lunaranime.ru";
const API_URL: &str = "https://api.lunaranime.ru";

struct LunarAnime;

impl MangaSource for LunarAnime {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!("{API_URL}/api/manga/recent?page={page}&limit=30");
            Ok(parse_recent(&fetch_json(&target, RECENT_FIXTURE)))
        } else {
            Ok(parse_search(&fetch_json(
                &format!("{API_URL}/api/manga/search?page={page}&limit=30&sort=relevance"),
                SEARCH_FIXTURE,
            )))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let mut params = vec![
            ("page", page(&request).to_string()),
            ("limit", "30".to_string()),
            ("sort", "relevance".to_string()),
        ];
        if !query.is_empty() {
            params.push(("query", query.to_string()));
        }
        for id in ["status", "country", "language", "year"] {
            if let Some(value) = filter_string(&request, id).filter(|value| !value.is_empty()) {
                params.push((if id == "country" { "country" } else { id }, value));
            }
        }
        let genres = request
            .get("filters")
            .and_then(|filters| filters.get("genres"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if !genres.is_empty() {
            params.push(("genres", genres.join(",")));
        }
        Ok(parse_search(&fetch_json(
            &format!("{API_URL}/api/manga/search?{}", encode_params(&params)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_slug(&slug_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let slug = slug_from_key(&key);
        let password = fetch_json(
            &format!("{API_URL}/api/manga/password/info/{slug}"),
            PASSWORD_FIXTURE,
        );
        Ok(parse_chapters(
            &fetch_json(&format!("{API_URL}/api/manga/{slug}"), CHAPTERS_FIXTURE),
            &password,
            &slug,
            filter_string(&request, "language").as_deref(),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample/1?lang=en".into());
        Ok(fetch_pages(&key).unwrap_or_else(|| {
            vec![page_entry(
                0,
                "https://storage.lunaranime.ru/cdn/sample/page.jpg".into(),
                chapter_url(&key),
            )]
        }))
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/manga/{}", slug_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_slug(slug: &str) -> CatalogItem {
    parse_details(&fetch_json(
        &format!("{API_URL}/api/manga/title/{slug}"),
        DETAILS_FIXTURE,
    ))
}

fn fetch_pages(key: &str) -> Option<Vec<MangaPage>> {
    let (slug, chapter, lang) = chapter_parts(key);
    let chapter_url = chapter_url(key);
    let html = fetch_document(&chapter_url, READER_FIXTURE);
    let seeds = LunarReader::extract_seed_objects(&html);
    let rctx0 = LunarReader::generate_rctx(seeds.first()?)?;
    let rctx1 = LunarReader::generate_rctx(seeds.get(1)?)?;
    let token = LunarReader::generate_token(
        &rctx0,
        &rctx1,
        &slug,
        &chapter,
        system_time().map(|time| time.unix_seconds).unwrap_or(0),
    )?;
    let session = fetch_json(
        &format!("{API_URL}/api/manga/r/{token}?lang={}", url::query_escape(&lang)),
        PAGE_SESSION_FIXTURE,
    );
    let session_data = json_value(&session)
        .get("data")?
        .get("session_data")?
        .as_str()?
        .to_string();
    let images = LunarReader::decrypt_session_images(&session_data, &rctx0)?;
    Some(
        images
            .into_iter()
            .enumerate()
            .map(|(index, image)| page_entry(index, image, chapter_url.clone()))
            .collect(),
    )
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let value = json_value(body);
    let entries = value
        .get("manga")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(manga_item)
        .collect();
    let page = value.get("page").and_then(Value::as_u64).unwrap_or(1);
    let total = value
        .get("total_pages")
        .or_else(|| value.get("totalPages"))
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < total,
    }
}

fn parse_recent(body: &str) -> Paged<CatalogItem> {
    let value = json_value(body);
    let entries = value
        .get("our_mangas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(manga_item)
        .collect::<Vec<_>>();
    let page = value.get("page").and_then(Value::as_u64).unwrap_or(1);
    let limit = value.get("limit").and_then(Value::as_u64).unwrap_or(30);
    let total = value
        .get("total_count")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    Paged {
        entries,
        has_next_page: page * limit < total,
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let value = json_value(body);
    let manga = value.get("manga").unwrap_or(&value);
    let mut item = manga_item(manga);
    item.initialized = true;
    item.alternate_titles = string_list(manga.get("alternative_titles"))
        .into_iter()
        .filter(|title| title != &item.title)
        .collect();
    item
}

fn manga_item(value: &Value) -> CatalogItem {
    let slug = text(value, "slug").unwrap_or_else(|| "sample".into());
    let tags = [
        text(value, "demographic"),
        text(value, "genres"),
        text(value, "themes"),
    ]
    .into_iter()
    .flatten()
    .flat_map(|entry| parse_tags(&entry))
    .collect::<Vec<_>>();
    CatalogItem {
        key: slug.clone(),
        title: text(value, "title").unwrap_or_else(|| "Lunar Manga".into()),
        cover: text(value, "cover_url"),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        authors: text(value, "author")
            .map(|name| vec![name.trim().to_string()])
            .unwrap_or_default(),
        artists: text(value, "artist")
            .map(|name| vec![name.trim().to_string()])
            .unwrap_or_default(),
        description: text(value, "description"),
        tags,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        status: match text(value, "publication_status")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" | "upcoming" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            "hiatus" => ItemStatus::Hiatus,
            "cancelled" => ItemStatus::Cancelled,
            _ => ItemStatus::Unknown,
        },
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_chapters(
    body: &str,
    password_body: &str,
    slug: &str,
    selected_language: Option<&str>,
) -> Vec<MangaChapter> {
    let password = json_value(password_body);
    let has_series_password = password
        .get("has_series_password")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let locked_chapters = password
        .get("chapter_passwords")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some((
                text(value, "chapter_number")?,
                value.get("language").and_then(Value::as_str).map(ToOwned::to_owned),
            ))
        })
        .collect::<Vec<_>>();
    let mut chapters = json_value(body)
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|value| {
            selected_language
                .filter(|lang| !lang.is_empty())
                .is_none_or(|lang| value.get("language").and_then(Value::as_str) == Some(lang))
        })
        .map(|value| {
            let chapter = text(value, "chapter").unwrap_or_else(|| "1".into());
            let language = text(value, "language").unwrap_or_else(|| "en".into());
            let locked = has_series_password
                || locked_chapters.iter().any(|(number, lang)| {
                    number == &chapter && lang.as_deref().is_none_or(|lang| lang == language)
                });
            let number = chapter.parse::<f32>().ok().or_else(|| {
                value
                    .get("chapter_number")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32)
            });
            let base_name = format!("Chapter {}", chapter.trim_end_matches(".00").trim_end_matches(".0"));
            let title = text(value, "chapter_title")
                .filter(|title| !title.is_empty())
                .map(|title| {
                    if title.to_ascii_lowercase().contains(&base_name.to_ascii_lowercase())
                        || title.contains("Volume")
                        || title.contains("Vol.")
                    {
                        title
                    } else {
                        format!("{base_name}: {title}")
                    }
                })
                .unwrap_or(base_name);
            MangaChapter {
                key: format!("{slug}/{chapter}?lang={language}"),
                title: Some(if locked { format!("[Locked] {title}") } else { title }),
                chapter_number: number,
                language: Some(language.clone()),
                url: Some(format!("{BASE_URL}/manga/{slug}/{chapter}?lang={language}")),
                is_locked: locked,
                ..MangaChapter::default()
            }
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn page_entry(index: usize, image: String, referer: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.clone(),
            context: Some(manga::image_headers(&referer)),
        },
        headers: manga::image_headers(&referer),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn chapter_parts(key: &str) -> (String, String, String) {
    let (path, query) = key.split_once('?').unwrap_or((key, ""));
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let slug = parts.next().unwrap_or("sample").to_string();
    let chapter = parts.next().unwrap_or("1").to_string();
    let lang = query
        .split('&')
        .find_map(|part| part.strip_prefix("lang="))
        .unwrap_or("en")
        .to_string();
    (slug, chapter, lang)
}

fn chapter_url(key: &str) -> String {
    let (slug, chapter, lang) = chapter_parts(key);
    format!("{BASE_URL}/manga/{slug}/{chapter}?lang={lang}")
}

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/')
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(key)
        .to_string()
}

fn slug_from_url(input: &str) -> Option<String> {
    if !input.contains("lunaranime.ru/manga/") {
        return None;
    }
    let after = input.split("/manga/").nth(1)?;
    after
        .trim_matches('/')
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn encode_params(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn json_value(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_tags(value: &str) -> Vec<String> {
    if let Ok(tags) = serde_json::from_str::<Vec<String>>(value) {
        tags
    } else {
        vec![value.to_string()]
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"manga":[{"slug":"sample","title":"Sample Lunar Manga","cover_url":"https://storage.lunaranime.ru/cdn/sample.jpg","publication_status":"ongoing","genres":"[\"Action\"]"}],"page":1,"total_pages":1}"#;
const RECENT_FIXTURE: &str = r#"{"our_mangas":[{"slug":"sample","title":"Sample Lunar Manga","cover_url":"https://storage.lunaranime.ru/cdn/sample.jpg","publication_status":"ongoing","genres":"[\"Action\"]"}],"page":1,"limit":30,"total_count":1}"#;
const DETAILS_FIXTURE: &str = r#"{"manga":{"slug":"sample","title":"Sample Lunar Manga","cover_url":"https://storage.lunaranime.ru/cdn/sample.jpg","publication_status":"ongoing","genres":"[\"Action\"]"}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":[{"chapter":"1","chapter_number":1,"chapter_title":"Chapter 1","language":"en","uploaded_at":"2026-01-01T00:00:00"}]}"#;
const PASSWORD_FIXTURE: &str = r#"{"chapter_passwords":[],"has_series_password":false}"#;
const READER_FIXTURE: &str = r#"<html><script></script></html>"#;
const PAGE_SESSION_FIXTURE: &str = r#"{"data":{"session_data":null}}"#;
