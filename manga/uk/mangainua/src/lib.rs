use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, Viewer, abi::ExtensionResult, export_manga_source, http::HttpClient,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: MangaInUa = MangaInUa;
const BASE_URL: &str = "https://manga.in.ua";

struct MangaInUa;

impl MangaSource for MangaInUa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "date;desc"
        } else {
            "news_read;desc"
        };
        let target = filter_url(page(&request), sort, &Value::Null);
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            false,
            &Value::Null,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = client(BASE_URL)
                .post(format!("{BASE_URL}/index.php?do=search"))
                .form(&[
                    ("do", "search"),
                    ("subaction", "search"),
                    ("full_search", "1"),
                    ("story", query),
                    ("search_start", &page(&request).to_string()),
                    (
                        "result_from",
                        &(1 + 12 * (page(&request).saturating_sub(1))).to_string(),
                    ),
                ])
                .send_text()
                .unwrap_or_else(|_| LIST_FIXTURE.to_string());
            return Ok(parse_listing(
                &body,
                true,
                request.get("preferences").unwrap_or(&Value::Null),
            ));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sort").unwrap_or_else(|| "news_read;desc".to_string());
        Ok(parse_listing(
            &fetch_document(&filter_url(page(&request), &sort, filters), LIST_FIXTURE),
            false,
            request.get("preferences").unwrap_or(&Value::Null),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".to_string());
        let details_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&details_url, DETAILS_FIXTURE);
        let ajax = fetch_chapters_ajax(&body, &details_url);
        Ok(parse_chapters(&ajax))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/chapter-1.html".to_string());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_PAGE_FIXTURE);
        let ajax = fetch_images_ajax(&body, &chapter_url);
        Ok(parse_pages(&ajax, &chapter_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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

export_manga_source!(SOURCE);

fn client(referer: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer.to_string())
        .with_header("Accept-Language", "uk-UA,uk;q=0.9,en-US;q=0.8,en;q=0.7")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn ajax_client(referer: &str) -> HttpClient {
    client(referer).with_header("X-Requested-With", "XMLHttpRequest")
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client(target)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_url(page: u64, sort: &str, filters: &Value) -> String {
    let mut segments = Vec::new();
    if let Some(tags) = filter_string(filters, "tags").filter(|value| !value.is_empty()) {
        segments.push(format!("cat={tags}"));
    }
    if let Some(tags) = filter_string(filters, "excludeTags").filter(|value| !value.is_empty()) {
        segments.push(format!("!cat={tags}"));
    }
    for (key, param) in [
        ("status", "b.tra"),
        ("type", "b.type"),
        ("age", "b.vik"),
        ("chapters", "c.lastchappr"),
        ("years", "c.yer"),
    ] {
        if let Some(value) = filter_string(filters, key).filter(|value| !value.is_empty()) {
            segments.push(format!("{param}={value}"));
        }
    }
    segments.push(format!("sort={sort}"));
    let mut target = format!("{BASE_URL}/filter/{}/", segments.join("/"));
    if page > 1 {
        target.push_str(&format!("page/{page}/"));
    }
    target
}

fn parse_listing(body: &str, from_search: bool, preferences: &Value) -> Paged<CatalogItem> {
    let hidden = if pref_bool(preferences, "hideSearchByTag") && from_search {
        preference_values(preferences, "hiddenTags")
    } else {
        Vec::new()
    };
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("item"))
        .filter_map(|chunk| {
            if !hidden.is_empty()
                && hidden.iter().any(|tag| {
                    chunk.contains(&format!("cat/{tag}")) || chunk.contains(&format!(">{tag}<"))
                })
            {
                return None;
            }
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "card__title", "</h3>")
                .map(|text| html::strip_tags(&text))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "MANGA/in/UA".to_string())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("uk".to_string()),
                content_rating: Some("safe".to_string()),
                viewer: Some(Viewer::RightToLeft),
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("Наступна"),
    }
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let key = normalize_key(fallback_key);
    let title = html::text_between(body, "UAName", "</")
        .map(|text| html::strip_tags(&text))
        .filter(|value| !value.is_empty())
        .or_else(|| html::text_between(body, "<h1", "</h1>").map(|text| html::strip_tags(&text)))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "MANGA/in/UA".to_string()));
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "item__full-sidebar--poster", "data-src")
            .or_else(|| html::attr_after(body, "item__full-sidebar--poster", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        description: html::text_between(body, "item__full-description", "</div>")
            .map(|text| html::strip_tags(&text)),
        tags: info_values(body, "Жанри:"),
        language: Some("uk".to_string()),
        content_rating: Some("safe".to_string()),
        status: match info_value(body, "Статус перекладу:").as_deref() {
            Some("Триває") => ItemStatus::Ongoing,
            Some("Заморожено") => ItemStatus::Hiatus,
            Some("Покинуто") => ItemStatus::Cancelled,
            Some("Закінчений") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        viewer: Some(Viewer::RightToLeft),
        ..CatalogItem::default()
    }
}

fn fetch_chapters_ajax(body: &str, referer: &str) -> String {
    let Some(hash) = parse_user_hash(body) else {
        return CHAPTERS_FIXTURE.to_string();
    };
    let endpoint = "engine/ajax/controller.php?mod=load_chapters";
    let hash_query = parse_hash_query(body, endpoint).unwrap_or_else(|| "user_hash".to_string());
    let news_id = attr_near(body, "linkstocomics", "data-news_id").unwrap_or_default();
    let news_category = attr_near(body, "linkstocomics", "data-news_category").unwrap_or_default();
    let this_link = attr_near(body, "linkstocomics", "data-this_link").unwrap_or_default();
    ajax_client(referer)
        .post(format!("{BASE_URL}/{endpoint}"))
        .form(&[
            ("action", "show"),
            ("news_id", news_id.as_str()),
            ("news_category", news_category.as_str()),
            ("this_link", this_link.as_str()),
            (hash_query.as_str(), hash.as_str()),
        ])
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string())
}

fn fetch_images_ajax(body: &str, referer: &str) -> String {
    let Some(hash) = parse_user_hash(body) else {
        return PAGES_FIXTURE.to_string();
    };
    let endpoint = "engine/ajax/controller.php?mod=load_chapters_image";
    let hash_query = parse_hash_query(body, endpoint).unwrap_or_else(|| "user_hash".to_string());
    let news_id = attr_near(body, "comics", "data-news_id").unwrap_or_default();
    let target = format!("{BASE_URL}/{endpoint}&news_id={news_id}&action=show&{hash_query}={hash}");
    ajax_client(referer)
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("ltcitems")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let chapter_number =
                html::attr(chunk, "manga-chappter").and_then(|value| value.parse::<f32>().ok());
            let volume = html::attr(chunk, "manga-tom").unwrap_or_default();
            let text = html::text_between(chunk, "<a", "</a")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let title = if text.contains("Альтернативний") {
                Some(format!(
                    "Том {volume}. Розділ {}",
                    chapter_number.unwrap_or_default()
                ))
            } else {
                Some(text)
            };
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number,
                scanlators: html::attr(chunk, "translate")
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect(),
                date_uploaded: first_date(chunk),
                language: Some("uk".to_string()),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_user_hash(body: &str) -> Option<String> {
    body.split("site_login_hash = '")
        .nth(1)?
        .split('\'')
        .next()
        .map(ToString::to_string)
}

fn parse_hash_query(body: &str, endpoint: &str) -> Option<String> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains(endpoint))?;
    let marker = ": site_login_hash";
    let before = script.split(marker).next()?;
    before
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|part| !part.is_empty())
        .last()
        .map(ToString::to_string)
}

fn attr_near(body: &str, marker: &str, attr: &str) -> Option<String> {
    body.find(marker)
        .and_then(|index| html::attr(&body[index..], attr))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let start = body.find(label)?;
    html::text_between(&body[start..], "item__full-sidebar--description", "</")
        .map(|value| html::strip_tags(&value))
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    info_value(body, label)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn first_date(body: &str) -> Option<i64> {
    for part in body.split(|ch: char| !ch.is_ascii_digit() && ch != '.') {
        if part.len() == 10 && part.chars().nth(2) == Some('.') && part.chars().nth(5) == Some('.')
        {
            let mut pieces = part.split('.');
            let day = pieces.next()?;
            let month = pieces.next()?;
            let year = pieces.next()?;
            return dates::parse_ymd(&format!("{year}-{month}-{day}"));
        }
    }
    None
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find(BASE_URL) {
        return format!("/{}", input[index + BASE_URL.len()..].trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn pref_bool(preferences: &Value, key: &str) -> bool {
    preferences
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

const LIST_FIXTURE: &str = r#"<article class="item"><h3 class="card__title"><a href="https://manga.in.ua/sample.html">Sample MangaInUa</a></h3><img src="/cover.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<span class="UAName">Sample MangaInUa</span><div class="item__full-sidebar--poster"><img src="/cover.jpg"></div><div id="linkstocomics" data-news_id="1" data-news_category="2" data-this_link="/sample.html"></div><script>site_login_hash = 'hash'; var data = { user_hash: site_login_hash }; fetch('engine/ajax/controller.php?mod=load_chapters')</script>"#;
const CHAPTERS_FIXTURE: &str = r#"<div class="ltcitems" manga-chappter="1" manga-tom="1" translate="Team"><span>01.01.2024</span><a href="https://manga.in.ua/sample/chapter-1.html">Розділ 1</a></div>"#;
const PAGES_PAGE_FIXTURE: &str = r#"<div id="comics" data-news_id="1"></div><script>site_login_hash = 'hash'; var data = { user_hash: site_login_hash }; fetch('engine/ajax/controller.php?mod=load_chapters_image')</script>"#;
const PAGES_FIXTURE: &str = r#"<li><img data-src="https://manga.in.ua/page-1.jpg"></li>"#;
