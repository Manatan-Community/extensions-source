use base64::{Engine as _, engine::general_purpose};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Source = Source;
const BASE_URL: &str = "https://raijin-scans.fr";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(HOME_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing(&request) == "latest" {
            Ok(latest(page))
        } else {
            Ok(parse_popular(&fetch(BASE_URL, HOME_FIXTURE)))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page > 1 {
            format!(
                "{BASE_URL}/page/{page}/?s={}&post_type=wp-manga",
                url::query_escape(query)
            )
        } else {
            format!(
                "{BASE_URL}/?s={}&post_type=wp-manga",
                url::query_escape(query)
            )
        };
        Ok(parse_search(&fetch(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        Ok(parse_details(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        let show_premium = request
            .get("preferences")
            .and_then(|p| p.get("show_premium_chapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &url::join_url(BASE_URL, &key),
            show_premium,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/serie/sample/chapter-1".into());
        Ok(parse_pages(
            &fetch(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
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
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch(input, DETAILS_FIXTURE), Some(key))),
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
fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn latest(page: u64) -> Paged<CatalogItem> {
    let home = fetch(BASE_URL, HOME_FIXTURE);
    if page <= 1 {
        return parse_latest_home(&home);
    }
    let Some(nonce) = nonce(&home) else {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    };
    let page_text = (page - 1).to_string();
    let body = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "*/*")
        .header("Origin", BASE_URL)
        .form(&[
            ("action", "load_manga"),
            ("page", page_text.as_str()),
            ("nonce", nonce.as_str()),
        ])
        .send_text()
        .unwrap_or_else(|_| LATEST_AJAX_FIXTURE.to_string());
    parse_latest_ajax(&body)
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("swiper-slide")
            .skip(1)
            .filter_map(card)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}
fn parse_latest_home(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("recently-updated")
            .nth(1)
            .unwrap_or(body)
            .split("div class=\"unit")
            .skip(1)
            .filter_map(card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("load-more-manga"),
    }
}
fn parse_latest_ajax(body: &str) -> Paged<CatalogItem> {
    let root = json(body, LATEST_AJAX_FIXTURE);
    let html_body = root
        .pointer("/data/manga_html")
        .and_then(Value::as_str)
        .unwrap_or("");
    let current = root
        .pointer("/data/current_page")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let total = root
        .pointer("/data/total_pages")
        .and_then(Value::as_u64)
        .unwrap_or(current);
    Paged {
        entries: html_body
            .split("div class=\"unit")
            .skip(1)
            .filter_map(card)
            .fold(Vec::new(), push_unique),
        has_next_page: current < total,
    }
}
fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("div class=\"unit")
            .skip(1)
            .filter_map(card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") && !body.contains("disabled"),
    }
}

fn card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "c-title", "</a>")
            .or_else(|| html::text_between(chunk, "div.info", "</a>"))
            .or_else(|| html::attr_after(chunk, "<a", "title"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Raijin Scans".into()),
        cover: image(chunk).map(|img| url::join_url(BASE_URL, &img)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/serie/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "serie-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Raijin Scans".into()),
        cover: html::attr_after(body, "img class=\"cover", "src")
            .or_else(|| image(body))
            .map(|img| url::join_url(BASE_URL, &img)),
        description: description(body),
        authors: stat(body, "Auteur").into_iter().collect(),
        artists: stat(body, "Artiste").into_iter().collect(),
        tags: body
            .split("genre-link")
            .skip(1)
            .filter_map(|c| html::text_between(c, ">", "</"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect(),
        status: if stat(body, "État du titre")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("termin")
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_url: &str, show_premium: bool) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|c| c.contains("item"))
        .filter(|c| show_premium || !c.contains("cairo-premium"))
        .filter_map(|c| {
            let href = html::attr_after(c, "<a", "href")
                .unwrap_or_else(|| format!("{}/1", manga_url.trim_end_matches('/')));
            let premium = c.contains("cairo-premium");
            let key = normalize(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!(
                    "{}{}",
                    if premium { "[Premium] " } else { "" },
                    html::attr_after(c, "<a", "title")
                        .or_else(
                            || html::text_between(c, "<a", "</a>").map(|v| html::strip_tags(&v))
                        )
                        .unwrap_or_else(|| "Chapter".into())
                )),
                is_locked: premium,
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    if body.contains("subscription-required-message") || chapter_url.contains("connexion") {
        return Vec::new();
    }
    if let Some(pages) = ajax_pages(body, chapter_url).filter(|pages| !pages.is_empty()) {
        return pages;
    }
    body.split("<img")
        .skip(1)
        .filter_map(image)
        .filter(|img| !img.starts_with("data:"))
        .enumerate()
        .map(|(i, img)| page_item(url::join_url(BASE_URL, &img), i))
        .collect()
}

fn ajax_pages(body: &str, chapter_url: &str) -> Option<Vec<MangaPage>> {
    let script = body.split("<script").find(|c| c.contains("rjfr_"))?;
    let manifest = object_after(script, "push(")?;
    let root: Value = serde_json::from_str(&manifest).ok()?;
    let order = root.get("m")?.as_str()?.split('|').collect::<Vec<_>>();
    let parts = root.get("c")?.as_object()?;
    let b64 = order
        .into_iter()
        .filter_map(|k| parts.get(k)?.as_str())
        .collect::<Vec<_>>()
        .join("");
    let decoded = general_purpose::STANDARD.decode(b64).ok()?;
    let config: Value = serde_json::from_slice(&decoded).ok()?;
    let shuffled = config.get("d")?.as_array()?;
    let perm = config.get("m")?.as_array()?;
    let mut ordered = vec![Value::Null; shuffled.len()];
    for (i, p) in perm.iter().filter_map(Value::as_u64).enumerate() {
        if let Some(slot) = ordered.get_mut(p as usize) {
            *slot = shuffled.get(i).cloned().unwrap_or(Value::Null);
        }
    }
    let req = ordered
        .get(13)?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    let resp = ordered
        .get(14)?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    if req.len() < 10 || resp.len() < 10 {
        return None;
    }
    let action = ordered.get(12)?.as_str()?;
    let token = ordered.get(2)?.as_str()?;
    let instance = ordered.get(3)?.as_str()?;
    let manga_id = ordered.get(4)?.as_str()?;
    let chapter = chapter_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("1");
    let reader_root =
        html::attr_after(body, "data-rj-free-reader-root", "data-rj-free-reader-root")
            .unwrap_or_default();
    let mut offset = "0".to_string();
    let mut cursor = String::new();
    let mut pages = Vec::new();
    for _ in 0..100 {
        let response = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .header("Referer", chapter_url)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "*/*")
            .header("Origin", BASE_URL)
            .form(&[
                ("action", action),
                (req[0], ""),
                (req[1], token),
                (req[2], instance),
                (req[3], manga_id),
                (req[4], chapter),
                (req[5], "local"),
                (req[6], "0"),
                (req[7], offset.as_str()),
                (req[8], reader_root.as_str()),
                (req[9], cursor.as_str()),
            ])
            .send_text()
            .ok()?;
        let root = json(&response, "{}");
        let payload = root.get(resp[1])?.as_object()?;
        for img in payload.get(resp[2])?.as_array()? {
            if let Some(image_url) = img.get(resp[4]).and_then(Value::as_str) {
                pages.push(page_item(image_url.to_string(), pages.len()));
            }
        }
        offset = payload
            .get(resp[7])
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        cursor = payload
            .get(resp[8])
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !payload
            .get(resp[9])
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
    }
    Some(pages)
}

fn page_item(image: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}
fn object_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let object = rest.find('{')?;
    let b = rest.as_bytes();
    let mut i = object;
    let mut d = 0;
    let mut s = false;
    let mut e = false;
    while i < b.len() {
        let c = b[i];
        if s {
            if e {
                e = false;
            } else if c == b'\\' {
                e = true;
            } else if c == b'"' {
                s = false;
            }
        } else if c == b'"' {
            s = true;
        } else if c == b'{' {
            d += 1;
        } else if c == b'}' {
            d -= 1;
            if d == 0 {
                return Some(rest[object..=i].to_string());
            }
        }
        i += 1;
    }
    None
}
fn description(body: &str) -> Option<String> {
    body.split("content.innerHTML = `")
        .nth(1)
        .and_then(|r| r.split("`;").next())
        .map(html::strip_tags)
        .filter(|v| !v.is_empty())
        .or_else(|| {
            html::text_between(body, "description-content", "</div>")
                .map(|v| html::strip_tags(&v))
                .filter(|v| !v.is_empty())
        })
}
fn stat(body: &str, label: &str) -> Option<String> {
    body.split("stat-item")
        .find(|c| c.contains(label))
        .and_then(|c| html::text_between(c, "stat-value", "</"))
        .map(|v| html::strip_tags(&v))
        .filter(|v| !v.is_empty())
}
fn nonce(body: &str) -> Option<String> {
    body.split("\"nonce\"")
        .nth(1)?
        .split('"')
        .nth(2)
        .map(ToString::to_string)
}
fn image(body: &str) -> Option<String> {
    html::attr_after(body, "<img", "data-src")
        .or_else(|| html::attr_after(body, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}
fn normalize(value: &str) -> String {
    if value.starts_with("http") {
        if let Some(i) = value.find("/serie/") {
            return format!("/{}", value[i + 1..].trim_end_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}
fn deeplink(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) && input.contains("/serie/")).then(|| normalize(input))
}
fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}
fn json(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or_else(|_| json!({}))
}
fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|e| e.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"<script id="ajax-sh-js-extra">var ajax = {"nonce":"fixture"};</script><section id="most-viewed"><div class="swiper-slide unit"><a class="c-title" href="/serie/sample">Sample Raijin</a><img src="/cover.jpg"></div></section><section class="recently-updated"><div class="unit"><div class="info"><a href="/serie/sample">Sample Raijin</a></div><img src="/cover.jpg"></div></section><a id="load-more-manga"></a>"#;
const SEARCH_FIXTURE: &str = r#"<div class="original card-lg"><div class="unit"><div class="info"><a href="/serie/sample">Sample Raijin</a></div><img src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="serie-title">Sample Raijin</h1><img class="cover" src="/cover.jpg"><div class="stat-item"><span>Auteur</span><span class="stat-value">Author</span></div><div class="stat-item"><span>Artiste</span><span class="stat-value">Artist</span></div><div class="stat-item"><span>Etat du titre</span><span class="stat-value">En cours</span></div><div class="description-content">Summary</div><div class="genre-link">Action</div><ul class="scroll-sm"><li class="item"><a href="/serie/sample/chapter-1" title="Chapitre 1"><span>Chapitre 1</span></a></li></ul>"#;
const LATEST_AJAX_FIXTURE: &str = r#"{"success":true,"data":{"manga_html":"<div class=\"unit\"><div class=\"info\"><a href=\"/serie/sample\">Sample Raijin</a></div><img src=\"/cover.jpg\"></div>","current_page":1,"total_pages":1}}"#;
const PAGES_FIXTURE: &str =
    r#"<div class="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
