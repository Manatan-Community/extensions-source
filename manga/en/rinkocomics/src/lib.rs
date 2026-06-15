use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: RinkoComics = RinkoComics;
const BASE_URL: &str = "https://rinkocomics.com";
const CHAPTERS_PER_PAGE: usize = 10;

struct RinkoComics;

impl MangaSource for RinkoComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_pinned(HOME_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_comics_page(&fetch_document(
                &comics_url(page, "newest", ""),
                SEARCH_FIXTURE,
            )));
        }
        Ok(Paged {
            entries: parse_pinned(&fetch_document(BASE_URL, HOME_FIXTURE)),
            has_next_page: false,
        })
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
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let sort = filter_value(&request, "sort").unwrap_or("newest");
        Ok(parse_comics_page(&fetch_document(
            &comics_url(page, sort, query),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        let hide_locked = preference_bool(&request, "hide_paid_chapters");
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body, hide_locked);
        if let (Some(comic_id), Some(nonce)) =
            (attr_value(&body, "data-comic-id"), extract_nonce(&body))
        {
            let mut offset = attr_value(&body, "data-offset")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(chapters.len());
            if offset == 0 || offset > chapters.len() {
                offset = chapters.len();
            }
            loop {
                let more = fetch_more_chapters(&comic_id, offset, &nonce, hide_locked);
                if more.is_empty() {
                    break;
                }
                merge_chapters(&mut chapters, more);
                offset += CHAPTERS_PER_PAGE;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/chapter-1".to_string());
        let key = key.trim_end_matches("#lock").to_string();
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"listingId": "popular", "page": 1}))?;
        let latest = self.list(serde_json::json!({"listingId": "latest", "page": 1}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Pinned".into(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: false,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| url::join_url(BASE_URL, key.trim_end_matches("#lock"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_more_chapters(
    comic_id: &str,
    offset: usize,
    nonce: &str,
    hide_locked: bool,
) -> Vec<MangaChapter> {
    let body = client()
        .post(&format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .xhr()
        .form(&[
            ("action", "load_more_chapters"),
            ("nonce", nonce),
            ("comic_id", comic_id),
            ("offset", &offset.to_string()),
        ])
        .send_text()
        .unwrap_or_default();
    serde_json::from_str::<AjaxResponse>(&body)
        .ok()
        .and_then(|response| response.data)
        .and_then(|data| data.html)
        .map(|html| parse_chapters(&html, hide_locked))
        .unwrap_or_default()
}

fn comics_url(page: u64, sort: &str, query: &str) -> String {
    let path = if page <= 1 {
        "comic/".to_string()
    } else {
        format!("comic/page/{page}/")
    };
    let mut params = vec![("post_type", "comic".to_string())];
    if !query.trim().is_empty() {
        params.push(("s", url::query_escape(query.trim())));
    }
    if !sort.is_empty() {
        params.push(("sort", sort.to_string()));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/{path}?{query}")
}

fn parse_pinned(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("pinned-comic-card"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "pinned-comic-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))?;
            let key = normalize_key(&href);
            Some(catalog_item(key, title, image_attr(chunk), false))
        })
        .collect()
}

fn parse_comics_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter(|chunk| chunk.contains("ac-card"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "ac-title", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "ac-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href))?;
                let key = normalize_key(&href);
                Some(catalog_item(key, title, image_attr(chunk), false))
            })
            .collect(),
        has_next_page: body.contains("ac-pagination") && body.contains("next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comic/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "comic-info-upper", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Rinko Comics".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: graph_values(body).into_iter().take(1).collect(),
        artists: graph_values(body).into_iter().skip(1).take(1).collect(),
        tags: link_texts(body, "genre"),
        description: html::text_between(body, "comic-synopsis", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(
            &html::text_between(body, "comic-status", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = attr_value(chunk, "data-permalink")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let locked = is_locked(chunk);
            if locked && hide_locked {
                return None;
            }
            let mut title = html::text_between(chunk, "chapter-number", "</")
                .or_else(|| attr_value(chunk, "data-title"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            if locked {
                title = format!("Locked - {title}");
            }
            let mut key = normalize_key(&href);
            if locked {
                key.push_str("#lock");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, key.trim_end_matches("#lock"))),
                date_uploaded: html::text_between(chunk, "chapter-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-image"))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn graph_values(body: &str) -> Vec<String> {
    body.split("comic-graph")
        .skip(1)
        .flat_map(|chunk| chunk.split("<span").skip(1).take(4))
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "-")
        .collect()
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("completed") => ItemStatus::Completed,
        text if text.contains("cancel") => ItemStatus::Cancelled,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Ongoing,
    }
}

fn is_locked(chunk: &str) -> bool {
    let reason = attr_value(chunk, "data-reason")
        .unwrap_or_default()
        .to_ascii_lowercase();
    (!reason.is_empty() && reason != "free")
        || chunk.contains("locked-chapter")
        || chunk.contains("chapter_price")
        || html::attr_after(chunk, "<a", "href")
            .is_none_or(|href| href.trim().is_empty() || href == "#")
}

fn merge_chapters(chapters: &mut Vec<MangaChapter>, more: Vec<MangaChapter>) {
    for chapter in more {
        if !chapters.iter().any(|existing| existing.key == chapter.key) {
            chapters.push(chapter);
        }
    }
}

fn extract_nonce(body: &str) -> Option<String> {
    let marker = "\"nonce\"";
    let start = body.find(marker)?;
    html::attr(&body[start..], "nonce").or_else(|| {
        let rest = &body[start + marker.len()..];
        rest.split('"').nth(1).map(ToString::to_string)
    })
}

fn normalize_key(value: &str) -> String {
    let value = value
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(path) = value.strip_prefix(BASE_URL) {
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn attr_value(input: &str, name: &str) -> Option<String> {
    html::attr(input, name).or_else(|| html::attr_after(input, name, name))
}

fn filter_value<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
}

fn preference_bool(request: &Value, key: &str) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct AjaxResponse {
    data: Option<AjaxData>,
}

#[derive(Debug, Deserialize)]
struct AjaxData {
    html: Option<String>,
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
<a class="pinned-comic-card" href="/comic/sample"><div class="pinned-comic-title">Sample Rinko</div><img data-src="/cover.jpg"></a>
"#;
const SEARCH_FIXTURE: &str = r#"
<article class="ac-card"><div class="ac-title"><a href="/comic/sample">Sample Rinko</a></div><div class="ac-thumb"><img data-src="/cover.jpg"></div></article>
<div class="ac-pagination"><a class="next" href="/comic/page/2/">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="comic-info-upper"><h1>Sample Rinko</h1></div>
<meta property="og:image" content="/cover.jpg">
<div class="comic-graph"><span>Writer</span><span>Artist</span></div>
<div class="comic-status"><span>Ongoing</span></div>
<div class="comic-genres"><a class="genre">Fantasy</a></div>
<div class="comic-synopsis">A sample comic.</div>
<script>var comicworld_ajax = {"nonce":"sample-nonce"};</script>
<button id="loadMoreChaptersBtn" data-comic-id="42" data-offset="1"></button>
<li class="chapter" data-permalink="/comic/sample/chapter-1"><span class="chapter-number">Chapter 1</span><span class="chapter-date">Jan 1, 2024</span><a href="/comic/sample/chapter-1">Read</a></li>
<li class="chapter locked-chapter" data-permalink="/comic/sample/chapter-2" data-reason="paid"><span class="chapter-number">Chapter 2</span></li>
"#;
const PAGES_FIXTURE: &str = r#"
<img class="chapter-image" data-src="/pages/1.jpg">
<img class="chapter-image" data-src="/pages/2.jpg">
"#;
