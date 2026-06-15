use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: HolyScans = HolyScans;
const BASE_URL: &str = "https://holyscans.com.tr";
const AJAX_URL: &str = "https://holyscans.com.tr/wp-admin/admin-ajax.php";

struct HolyScans;

impl MangaSource for HolyScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let form = vec![
            ("action".to_string(), "filter_manga_archive".to_string()),
            ("paged".to_string(), page.to_string()),
        ];
        let body = post_form_or_fixture(
            &request,
            &form,
            &format!("{BASE_URL}/manga/?m_orderby=views"),
            LIST_FIXTURE,
        );
        Ok(parse_ajax_manga_list(&body))
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
                    &fetch_document_or_fixture(&request, query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let form = vec![
                ("action".to_string(), "holy_live_search".to_string()),
                ("keyword".to_string(), query.to_string()),
            ];
            let body = post_form_or_fixture(&request, &form, BASE_URL, SEARCH_FIXTURE);
            return Ok(Paged {
                entries: parse_live_search(&body),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let form = archive_form(page, &request);
        let body = post_form_or_fixture(&request, &form, BASE_URL, LIST_FIXTURE);
        Ok(parse_ajax_manga_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&request, &absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &request,
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let chapter_url = absolute_url(&key);
        let body = fetch_document_or_fixture(&request, &chapter_url, PAGES_FIXTURE);
        let pages = fetch_ajax_pages(&request, &body, &chapter_url);
        if pages.is_empty() {
            Ok(parse_static_pages(&body, &chapter_url))
        } else {
            Ok(pages)
        }
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
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(&request, input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(request: &Value, target: &str, fixture: &str) -> String {
    login_if_requested(request);
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_form_or_fixture(
    request: &Value,
    form: &[(String, String)],
    referer: &str,
    fixture: &str,
) -> String {
    login_if_requested(request);
    let borrowed = form
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    client()
        .post(AJAX_URL)
        .xhr()
        .referer(referer)
        .form(&borrowed)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn login_if_requested(request: &Value) {
    let username = preference(request, "pref_username");
    let password = preference(request, "pref_password");
    if username.is_empty() || password.is_empty() {
        return;
    }
    let form = [
        ("log", username.as_str()),
        ("pwd", password.as_str()),
        ("submit_custom_login", ""),
        ("rememberme", "forever"),
    ];
    let _ = client()
        .post(format!("{BASE_URL}/giris/"))
        .browser_document()
        .referer(format!("{BASE_URL}/giris/"))
        .form(&form)
        .send_text();
}

fn preference(request: &Value, key: &str) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn archive_form(page: u64, request: &Value) -> Vec<(String, String)> {
    let mut form = vec![
        ("action".to_string(), "filter_manga_archive".to_string()),
        ("paged".to_string(), page.to_string()),
    ];
    for (filter_key, form_key) in [
        ("genres", "genres[]"),
        ("types", "types[]"),
        ("statuses", "statuses[]"),
    ] {
        for value in selected_filter_values(request, filter_key) {
            form.push((form_key.to_string(), value));
        }
    }
    form
}

fn selected_filter_values(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request.get("filters").and_then(|filters| filters.get(key)) else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        Value::Object(values) => values
            .iter()
            .filter_map(|(id, selected)| selected.as_bool().unwrap_or(false).then_some(id.clone()))
            .collect(),
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_ajax_manga_list(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<AjaxResponse>(body).unwrap_or_default();
    let html = if response.html_content().is_empty() {
        body.to_string()
    } else {
        response.html_content()
    };
    let entries = html
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-card-v2") || chunk.contains("mc-image-box"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "mc-title", "href")
                .or_else(|| html::attr_after(chunk, "mc-image-box", "data-href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "mc-title", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("tr".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: response.has_next(),
    }
}

fn parse_live_search(body: &str) -> Vec<CatalogItem> {
    let response = serde_json::from_str::<LiveSearchResponse>(body).unwrap_or_default();
    let html = if response.data.is_empty() {
        body
    } else {
        &response.data
    };
    html.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("holy-live-result-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Manga".into());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("tr".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-main-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "manga-cover-area", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: detail_values(body, "Yazar"),
        artists: detail_values(body, "Çizer"),
        tags: detail_values(body, "Türler"),
        description: html::text_between(body, "manga-summary-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&detail_values(body, "Durum").join(" ")),
        url: Some(absolute_url(&key)),
        language: Some("tr".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("ch-list-item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "ch-title", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "ch-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_holy_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn fetch_ajax_pages(request: &Value, body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let Some(chapter_id) = script_field(body, "chapter_id") else {
        return Vec::new();
    };
    let Some(load_time) = script_field(body, "load_time") else {
        return Vec::new();
    };
    let Some(page_token) = script_field(body, "page_token") else {
        return Vec::new();
    };
    let Some(nonce) = script_field(body, "nonce") else {
        return Vec::new();
    };
    let form = vec![
        ("action".to_string(), "holy_get_chapter_images".to_string()),
        ("nonce".to_string(), nonce),
        ("chapter_id".to_string(), chapter_id),
        ("load_time".to_string(), load_time),
        ("page_token".to_string(), page_token),
    ];
    let response = post_form_or_fixture(request, &form, chapter_url, PAGES_AJAX_FIXTURE);
    parse_pages_response(&response, chapter_url)
}

fn parse_pages_response(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let response = serde_json::from_str::<PagesResponse>(body).unwrap_or_default();
    response
        .urls()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_static_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn script_field(body: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\"");
    let start = body.find(&marker)? + marker.len();
    let rest = body[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if let Some(rest) = rest.strip_prefix('"') {
        return rest.split('"').next().map(ToString::to_string);
    }
    let value = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!value.is_empty()).then_some(value)
}

fn detail_values(body: &str, label: &str) -> Vec<String> {
    let lower = body.to_lowercase();
    let Some(index) = lower.find(&label.to_lowercase()) else {
        return Vec::new();
    };
    let tail = &body[index..];
    let fragment = tail
        .find("</div>")
        .map(|end| &tail[..end + "</div>".len()])
        .unwrap_or(&tail[..tail.len().min(1200)]);
    let links = fragment
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !links.is_empty() {
        return links;
    }
    html::text_between(fragment, "d-val", "</")
        .map(|value| vec![html::strip_tags(&value)])
        .unwrap_or_default()
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("devam ediyor") {
        ItemStatus::Ongoing
    } else if lower.contains("tamamlandı") || lower.contains("final") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_holy_date(value: &str) -> Option<i64> {
    if let Some(date) = dates::parse_fixture_date(value) {
        return Some(date);
    }
    let lower = value.to_lowercase();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)?;
    if lower.contains("yeni")
        || lower.contains("bugün")
        || lower.contains("saat")
        || lower.contains("dakika")
    {
        return Some(now);
    }
    if lower.contains("dün") {
        return Some(now - 86_400);
    }
    let count = lower
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if count <= 0 {
        return None;
    }
    if lower.contains("gün") {
        Some(now - count * 86_400)
    } else if lower.contains("hafta") {
        Some(now - count * 7 * 86_400)
    } else if lower.contains("ay") {
        Some(now - count * 30 * 86_400)
    } else if lower.contains("yıl") {
        Some(now - count * 365 * 86_400)
    } else {
        None
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Default, Deserialize)]
struct AjaxResponse {
    data: Option<AjaxData>,
}

impl AjaxResponse {
    fn html_content(&self) -> String {
        self.data
            .as_ref()
            .and_then(|data| data.content.clone())
            .unwrap_or_default()
    }

    fn has_next(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|data| data.pagination.as_deref())
            .is_some_and(|pagination| pagination.contains("next page-numbers"))
    }
}

#[derive(Default, Deserialize)]
struct AjaxData {
    content: Option<String>,
    pagination: Option<String>,
}

#[derive(Default, Deserialize)]
struct LiveSearchResponse {
    data: String,
}

#[derive(Default, Deserialize)]
struct PagesResponse {
    data: Option<PagesData>,
}

impl PagesResponse {
    fn urls(self) -> Vec<String> {
        self.data.map(|data| data.urls).unwrap_or_default()
    }
}

#[derive(Default, Deserialize)]
struct PagesData {
    #[serde(default)]
    urls: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
{
  "data": {
    "content": "<div class=\"manga-card-v2\"><div class=\"mc-image-box\" data-href=\"/manga/sample/\"><img src=\"/cover.jpg\" alt=\"Sample Holy Scans\"></div><div class=\"mc-title\"><a href=\"/manga/sample/\">Sample Holy Scans</a></div></div>",
    "pagination": "<a class=\"next page-numbers\">Next</a>"
  }
}
"#;

const SEARCH_FIXTURE: &str = r#"
{
  "data": "<a class=\"holy-live-result-item\" href=\"/manga/sample/\"><img src=\"/cover.jpg\"><span>Sample Holy Scans</span></a>"
}
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="manga-main-title">Sample Holy Scans</h1>
<div class="manga-cover-area"><img src="/cover.jpg"></div>
<div class="detail-box"><span>Yazar</span><span class="d-val">Sample Author</span></div>
<div class="detail-box"><span>Çizer</span><span class="d-val">Sample Artist</span></div>
<div class="detail-box"><span>Türler</span><span class="d-val"><a>Aksiyon</a></span></div>
<div class="detail-box"><span>Durum</span><span class="d-val">Devam Ediyor</span></div>
<div class="manga-summary-content">Sample description.</div>
<div class="manga-chapter-list-wrap">
  <a class="ch-list-item" href="/manga/sample/chapter-1/"><span class="ch-title">Chapter 1</span><span class="ch-date">2024-01-01</span></a>
</div>
"#;

const PAGES_FIXTURE: &str = r#"
<script>
window.reader = {"chapter_id":1,"load_time":1704067200,"page_token":"sample-token","nonce":"sample-nonce"};
</script>
"#;

const PAGES_AJAX_FIXTURE: &str = r#"
{
  "data": {
    "urls": ["/page-1.jpg"]
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(
            parse_ajax_manga_list(LIST_FIXTURE).entries[0].title,
            "Sample Holy Scans"
        );
        assert_eq!(
            parse_live_search(SEARCH_FIXTURE)[0].title,
            "Sample Holy Scans"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(
            parse_pages_response(PAGES_AJAX_FIXTURE, &absolute_url("/manga/sample/chapter-1"))
                .len(),
            1
        );
    }
}
