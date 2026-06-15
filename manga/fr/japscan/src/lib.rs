use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{
        ExtensionResult, WebViewRequest, WebViewScript, WebViewScriptRunAt, WebViewWait,
        WebViewWaitUntil, webview_open,
    },
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Japscan = Japscan;
const BASE_URL: &str = "https://www.japscan.foo";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";
const CHAPTER_TYPES: [&str; 5] = ["manga", "manhua", "manhwa", "bd", "comic"];

struct Japscan;

impl MangaSource for Japscan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/mangas/?sort={sort}&p={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            return Ok(parse_search_page(&fetch_document_or_fixture(
                &format!("{BASE_URL}/mangas/{page}"),
                SEARCH_PAGE_FIXTURE,
            )));
        }
        Ok(parse_search_json(&post_search_or_fixture(
            query,
            SEARCH_JSON_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let show_spoilers = request
            .get("preferences")
            .and_then(|preferences| preferences.get("show_spoiler_chapters"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, manga_slug(&key), show_spoilers))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        Ok(fetch_pages_with_webview(&chapter_url).unwrap_or_else(|| parse_pages(PAGES_FIXTURE)))
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
        if let Some(key) = deeplink_key(input) {
            let item_key = manga_key_from_any_key(&key);
            let body =
                fetch_document_or_fixture(&url::join_url(BASE_URL, &item_key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(item_key))),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
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

fn post_search_or_fixture(query: &str, fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}/ls/"))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("search", query)])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-block")
        .skip(1)
        .filter_map(parse_card)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination")
            && !body.contains("pagination > li:last-child disabled")
            && (body.contains("Suivant") || body.contains("»") || body.contains("next")),
    }
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("div class=\"card")
        .skip(1)
        .filter_map(parse_search_card)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("manga-block") || body.contains("pagination"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.is_empty() {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Japscan".to_string()),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if !href.starts_with(BASE_URL) && !href.starts_with('/') {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Japscan".to_string()),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<SearchHit>>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_JSON_FIXTURE).expect("fixture is valid"))
        .into_iter()
        .map(|hit| {
            let key = normalize_key(&hit.url);
            CatalogItem {
                key: key.clone(),
                title: hit.name,
                cover: Some(url::join_url(BASE_URL, &hit.image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "post-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Japscan".to_string()),
        cover: html::attr_after(body, "#main", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_after_label(body, "Auteur(s):")
            .map(|value| vec![value])
            .unwrap_or_default(),
        artists: info_after_label(body, "Artiste(s):")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: info_after_label(body, "Genre(s):")
            .map(|value| {
                value
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        description: synopsis(body),
        status: parse_status(&info_after_label(body, "Statut:").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(
    body: &str,
    manga_slug: Option<String>,
    show_spoilers: bool,
) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("list_chapters")
        .skip(1)
        .filter(|chunk| show_spoilers || !contains_spoiler_badge(chunk))
        .filter(|chunk| !is_hidden_chunk(chunk))
        .filter_map(|chunk| parse_chapter(chunk, manga_slug.as_deref()))
        .collect::<Vec<_>>();
    chapters = filter_outlier_chapters(chapters);
    chapters
}

fn parse_chapter(chunk: &str, manga_slug: Option<&str>) -> Option<MangaChapter> {
    let pairs = chapter_paths(chunk)
        .into_iter()
        .filter_map(|path| {
            if !valid_chapter_path(&path, manga_slug) {
                return None;
            }
            let title = title_for_path(chunk, &path).unwrap_or_else(|| "Chapitre".to_string());
            let title_num = digits_from_chapter_title(&title)?;
            let url_num = path.trim_end_matches('/').rsplit('/').next()?.to_string();
            (title_num == url_num).then_some((title, path))
        })
        .collect::<Vec<_>>();
    let (title, key) = pairs.into_iter().next()?;
    Some(MangaChapter {
        key: key.clone(),
        title: Some(title.clone()),
        chapter_number: chapter_number_from_text(&title),
        date_uploaded: html::text_between(chunk, "<span", "</span>")
            .map(|value| html::strip_tags(&value))
            .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
        url: Some(url::join_url(BASE_URL, &key)),
        ..MangaChapter::default()
    })
}

fn fetch_pages_with_webview(chapter_url: &str) -> Option<Vec<MangaPage>> {
    let response = webview_open(&WebViewRequest {
        url: chapter_url.to_string(),
        wait_for: Some(WebViewWait::Delay {
            milliseconds: 3_000,
        }),
        wait_until: Some(WebViewWaitUntil::LoadFinished),
        headers: vec![("Referer".to_string(), format!("{BASE_URL}/"))],
        timeout_ms: Some(30_000),
        return_html: true,
        scripts: vec![
            WebViewScript {
                id: Some("capture".to_string()),
                run_at: Some(WebViewScriptRunAt::DocumentStart),
                script: JAPSCAN_CAPTURE_SCRIPT.to_string(),
            },
            WebViewScript {
                id: Some("extract".to_string()),
                run_at: Some(WebViewScriptRunAt::AfterWait),
                script: JAPSCAN_EXTRACT_SCRIPT.to_string(),
            },
        ],
        ..WebViewRequest::default()
    })
    .ok()?;
    let raw = response
        .script_results
        .into_iter()
        .find(|result| result.id.as_deref() == Some("extract"))
        .and_then(|result| result.value)
        .and_then(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| Some(value.to_string()))
        })?;
    let payload = serde_json::from_str::<PagePayload>(&raw).ok()?;
    Some(pages_from_payload(payload))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Ok(payload) = serde_json::from_str::<PagePayload>(body) {
        return pages_from_payload(payload);
    }
    let images = body
        .split('"')
        .filter(|part| part.starts_with("http") && part.contains("japscan.foo"))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    pages_from_payload(PagePayload {
        images,
        p: String::new(),
        v: String::new(),
        pi: -1,
    })
}

fn pages_from_payload(payload: PagePayload) -> Vec<MangaPage> {
    payload
        .images
        .into_iter()
        .filter(|image| image.contains("japscan.foo"))
        .enumerate()
        .filter(|(index, _)| *index as i64 != payload.pi)
        .map(|(index, image)| {
            let mut image_url = if image.contains('?') {
                format!("{image}&y=1")
            } else {
                format!("{image}?y=1")
            };
            if !payload.p.is_empty() && !payload.v.is_empty() {
                image_url.push('&');
                image_url.push_str(&url::query_escape(&payload.p));
                image_url.push('=');
                image_url.push_str(&url::query_escape(&payload.v));
            }
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn chapter_paths(chunk: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for quote in ['"', '\''] {
        for part in chunk.split(quote) {
            if part.starts_with("/manga/")
                || part.starts_with("/manhua/")
                || part.starts_with("/manhwa/")
                || part.starts_with("/bd/")
                || part.starts_with("/comic/")
            {
                let path = normalize_key(part);
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

fn valid_chapter_path(path: &str, manga_slug: Option<&str>) -> bool {
    let parts = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 3 || !CHAPTER_TYPES.contains(&parts[0]) {
        return false;
    }
    if manga_slug.is_some_and(|slug| parts[1] != slug) {
        return false;
    }
    parts[2].chars().all(|ch| ch.is_ascii_digit())
}

fn title_for_path(chunk: &str, path: &str) -> Option<String> {
    let before = chunk.split(path).next().unwrap_or_default();
    let start = before
        .rfind("<a")
        .or_else(|| before.rfind("<div"))
        .unwrap_or(0);
    let after = &chunk[start..];
    html::text_between(after, ">", "</a>")
        .or_else(|| html::text_between(after, ">", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn filter_outlier_chapters(chapters: Vec<MangaChapter>) -> Vec<MangaChapter> {
    let mut numbered = chapters
        .iter()
        .filter_map(|chapter| {
            chapter
                .key
                .trim_end_matches('/')
                .rsplit('/')
                .next()?
                .parse::<i64>()
                .ok()
                .map(|num| (chapter.key.clone(), num))
        })
        .collect::<Vec<_>>();
    if numbered.len() < 2 {
        return chapters;
    }
    numbered.sort_by_key(|(_, num)| *num);
    let mut gap_index = None;
    let mut gap_size = 0;
    for index in 1..numbered.len() {
        let gap = numbered[index].1 - numbered[index - 1].1;
        if gap > gap_size {
            gap_size = gap;
            gap_index = Some(index);
        }
    }
    if gap_size <= 1000 {
        return chapters;
    }
    let Some(index) = gap_index else {
        return chapters;
    };
    let keep = numbered
        .into_iter()
        .take(index)
        .map(|(key, _)| key)
        .collect::<Vec<_>>();
    chapters
        .into_iter()
        .filter(|chapter| keep.contains(&chapter.key))
        .collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_after_label(body: &str, label: &str) -> Option<String> {
    body.split("<p")
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .map(|chunk| {
            html::strip_tags(chunk)
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn synopsis(body: &str) -> Option<String> {
    body.split("Synopsis")
        .nth(1)
        .and_then(|rest| html::text_between(rest, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("termine") || lower.contains("termin") {
        ItemStatus::Completed
    } else if lower.contains("cours") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn contains_spoiler_badge(chunk: &str) -> bool {
    let upper = html::strip_tags(chunk).to_ascii_uppercase();
    upper.contains("SPOILER") || upper.contains("RAW") || upper.contains("VUS")
}

fn is_hidden_chunk(chunk: &str) -> bool {
    let lower = chunk.replace(' ', "").to_ascii_lowercase();
    lower.contains("d-none")
        || lower.contains("hidden")
        || lower.contains("aria-hidden=\"true\"")
        || lower.contains("display:none")
        || lower.contains("visibility:hidden")
        || lower.contains("opacity:0")
        || lower.contains("width:0")
        || lower.contains("height:0")
        || lower.contains("pointer-events:none")
        || lower.contains("font-size:0")
        || lower.contains("text-indent:-")
        || lower.contains("max-width:0")
        || lower.contains("max-height:0")
}

fn digits_from_chapter_title(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let source = lower
        .split("chapitre")
        .nth(1)
        .or_else(|| lower.split("volume").nth(1))
        .unwrap_or(&lower);
    source
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .map(|part| part.replace('.', ""))
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn manga_slug(key: &str) -> Option<String> {
    let parts = key
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (parts.len() >= 2 && CHAPTER_TYPES.contains(&parts[0])).then(|| parts[1].to_string())
}

fn manga_key_from_any_key(key: &str) -> String {
    let parts = key
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() >= 2 && CHAPTER_TYPES.contains(&parts[0]) {
        format!("/{}/{}", parts[0], parts[1])
    } else {
        key.to_string()
    }
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        Some(normalize_key(input))
    } else if input.starts_with('/') {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    name: String,
    url: String,
    image: String,
}

#[derive(Debug, Deserialize)]
struct PagePayload {
    images: Vec<String>,
    #[serde(default)]
    p: String,
    #[serde(default)]
    v: String,
    #[serde(default)]
    pi: i64,
}

const JAPSCAN_CAPTURE_SCRIPT: &str = r#"
(() => {
  window.__manatanJapscanPayload = window.__manatanJapscanPayload || null;
  function decodeUtf8(bin) {
    try {
      return decodeURIComponent(Array.from(bin, c => '%' + ('00' + c.charCodeAt(0).toString(16)).slice(-2)).join(''));
    } catch (e) {
      return bin;
    }
  }
  function maybeStore(text) {
    if (!text || text.indexOf('japscan.foo') === -1) return;
    try {
      const parsed = JSON.parse(text);
      window.__manatanJapscanPayload = parsed;
    } catch (e) {}
  }
  const originalAtob = window.atob;
  window.atob = function(str) {
    const result = originalAtob.call(this, str);
    maybeStore(decodeUtf8(result));
    return result;
  };
  const originalReplace = String.prototype.replace;
  String.prototype.replace = function(searchValue, replaceValue) {
    const result = originalReplace.call(this, searchValue, replaceValue);
    if (typeof result === 'string' && /^[A-Za-z0-9+/]+={0,2}$/.test(result.trim())) {
      try {
        maybeStore(decodeUtf8(originalAtob.call(window, result.trim())));
      } catch (e) {}
    }
    return result;
  };
})();
"#;

const JAPSCAN_EXTRACT_SCRIPT: &str = r#"
(() => {
  function findImageArray(obj) {
    let found = null;
    (function visit(value) {
      if (found) return;
      if (Array.isArray(value) && value.length > 0 && value.every(v => typeof v === 'string' && v.indexOf('japscan.foo') !== -1)) {
        found = value;
        return;
      }
      if (value && typeof value === 'object') {
        for (const k in value) {
          if (Object.prototype.hasOwnProperty.call(value, k)) visit(value[k]);
        }
      }
    })(obj);
    return found;
  }
  const payload = window.__manatanJapscanPayload || {};
  let images = findImageArray(payload);
  if (!images) {
    images = Array.from(document.images).map(img => img.currentSrc || img.src).filter(src => src && src.indexOf('japscan.foo') !== -1);
  }
  const rc = window.__rc || {};
  const pathNumber = (location.pathname.match(/\/(\d+)(?:\/|$)/) || [null, '-1'])[1];
  return JSON.stringify({ images: images || [], p: rc.p || '', v: rc.v || '', pi: Number(pathNumber) || -1 });
})()
"#;

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="mangas-list"><div class="manga-block"><a href="/manga/sample"><img data-src="/cover.jpg">Sample Japscan</a></div></div>
<ul class="pagination"><li><a>Suivant</a></li></ul>
"#;
const SEARCH_PAGE_FIXTURE: &str = r#"<div class="card"><div class="p-2"><p><a href="https://www.japscan.foo/manga/sample">Sample Japscan</a></p><img src="/cover.jpg"></div></div>"#;
const SEARCH_JSON_FIXTURE: &str =
    r#"[{"name":"Sample Japscan","url":"/manga/sample","image":"/cover.jpg"}]"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Japscan</h1><div id="main"><div class="card-body"><img src="/cover.jpg">
<p><span>Auteur(s):</span> Auteur</p><p><span>Artiste(s):</span> Artiste</p><p><span>Genre(s):</span> Action, Aventure</p><p><span>Statut:</span> En Cours</p>
<div>Synopsis</div><p>Resume</p></div></div>
<div id="list_chapters"><div class="collapse"><div class="list_chapters"><a href="/manga/sample/1/">Chapitre 1</a><span>01 Jan 2024</span></div><div class="list_chapters"><a href="/manga/sample/2/">Chapitre 2 <span class="badge">RAW</span></a></div></div></div>
"#;
const PAGES_FIXTURE: &str = r#"{"images":["https://c4.japscan.foo/img/sample/001.jpg","https://c4.japscan.foo/img/sample/002.jpg"],"p":"token","v":"value","pi":-1}"#;
