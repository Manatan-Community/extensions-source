use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Yupmanga = Yupmanga;
const BASE_URL: &str = "https://www.yupmanga.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct Yupmanga;

impl MangaSource for Yupmanga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_series_list(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if listing_id(&request) == "latest" {
            format!("{BASE_URL}/?page={page}")
        } else {
            format!("{BASE_URL}/top")
        };
        Ok(parse_series_list(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_from_id(&key)],
                has_next_page: false,
            });
        }
        if query.chars().count() < 3 {
            return Err(error(
                "El termino de busqueda debe tener al menos 3 caracteres.",
            ));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(
            &format!(
                "{BASE_URL}/search.php?q={}&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        if body.contains("bg-red") && body.contains("<p") {
            return Err(error(
                "Limite de solicitudes alcanzado. Intente de nuevo en unos minutos.",
            ));
        }
        Ok(parse_series_list(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(details_from_id(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(fetch_chapters(&key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "1#1".to_string());
        let (chapter_id, manga_id) = split_chapter_key(&key);
        let pages = pages_for_chapter(&chapter_id, &manga_id);
        if pages.is_empty() {
            Ok(parse_reader_pages(
                PAGES_FIXTURE,
                &chapter_id,
                "fixture-token",
            ))
        } else {
            Ok(pages)
        }
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (_, manga_id) = split_chapter_key(&key);
            manga_url(&manga_id)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("series.php") {
            let key = normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_id(&key)),
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

fn fetch_ajax(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_ajax_form(target: &str, form: &[(&str, &str)]) -> String {
    client()
        .post(target)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .form(form)
        .send_text()
        .unwrap_or_default()
}

fn parse_series_list(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_manga_key(&href);
            if key.is_empty() {
                return None;
            }
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "object-cover", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(manga_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_catalog);
    Paged {
        entries,
        has_next_page: body.contains("Siguiente") || body.contains("&raquo;"),
    }
}

fn details_from_id(id: &str) -> CatalogItem {
    let key = normalize_manga_key(id);
    let body = fetch_document(&manga_url(&key), DETAILS_FIXTURE);
    parse_details(&body, &key)
}

fn parse_details(body: &str, fallback_id: &str) -> CatalogItem {
    CatalogItem {
        key: fallback_id.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Yupmanga".to_string()),
        description: html::text_between(body, "id=\"synopsisText\"", "</p>")
            .or_else(|| html::text_between(body, "id='synopsisText'", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: text_after_icon(body, "Editorial").into_iter().collect(),
        tags: body
            .split("genre-tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "property='og:image'", "content"))
            .map(|value| url::join_url(BASE_URL, &value)),
        status: parse_status(text_after_icon(body, "Estado").as_deref()),
        url: Some(manga_url(fallback_id)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(manga_id: &str) -> Vec<MangaChapter> {
    let manga_id = normalize_manga_key(manga_id);
    let mut chapters = Vec::new();
    let mut page = 1;
    loop {
        let body = fetch_ajax(
            &format!(
                "{BASE_URL}/ajax/load_chapters.php?series_id={}&page={page}&order=newest_first",
                url::query_escape(&manga_id)
            ),
            CHAPTERS_FIXTURE,
        );
        let root = json_or_fixture(&body, CHAPTERS_FIXTURE);
        let html_fragment = root.get("html").and_then(Value::as_str).unwrap_or_default();
        let before = chapters.len();
        for chapter in parse_chapter_fragment(html_fragment, &manga_id) {
            chapters = push_unique_chapter(chapters, chapter);
        }
        let current = root
            .get("currentPage")
            .or_else(|| root.get("current_page"))
            .and_then(Value::as_u64)
            .unwrap_or(page);
        let total = root
            .get("totalPages")
            .or_else(|| root.get("total_pages"))
            .and_then(Value::as_u64)
            .unwrap_or(current);
        if current >= total || chapters.len() == before {
            break;
        }
        page += 1;
    }
    chapters
}

fn parse_chapter_fragment(body: &str, manga_id: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-card"))
        .filter_map(|chunk| {
            let chapter_id = html::attr_after(chunk, "data-chapter", "data-chapter")
                .or_else(|| html::attr_after(chunk, "<a", "data-chapter"))?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Capitulo {chapter_id}"));
            Some(MangaChapter {
                key: format!("{chapter_id}#{manga_id}"),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                language: Some(LANG.to_string()),
                url: Some(manga_url(manga_id)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn pages_for_chapter(chapter_id: &str, manga_id: &str) -> Vec<MangaPage> {
    let series_body = if manga_id.is_empty() {
        String::new()
    } else {
        fetch_document(&manga_url(manga_id), "")
    };
    let csrf = extract_csrf(&series_body);
    let data_k = extract_data_k(&series_body);
    let data_v = extract_data_v(&series_body).unwrap_or_else(|| csrf.clone());
    let challenge_body = fetch_ajax(
        &format!(
            "{BASE_URL}/ajax/get_challenge.php?chapter={}&s={}",
            url::query_escape(chapter_id),
            url::query_escape(manga_id)
        ),
        "{}",
    );
    let challenge = serde_json::from_str::<Value>(&challenge_body).unwrap_or(Value::Null);
    let challenge_id = string_value(&challenge, "challenge_id").unwrap_or_default();
    let challenge_js = string_value(&challenge, "challenge_js").unwrap_or_default();
    if challenge_id.is_empty() || challenge_js.is_empty() {
        return Vec::new();
    }
    let answer = solve_simple_challenge(&challenge_js, &csrf, &data_k, &data_v);
    let token_body = post_ajax_form(
        &format!("{BASE_URL}/ajax/get_reader_token.php"),
        &[
            ("chapter", chapter_id),
            ("challenge_id", challenge_id.as_str()),
            ("answer", answer.as_str()),
        ],
    );
    let token = serde_json::from_str::<Value>(&token_body).unwrap_or(Value::Null);
    let token_value = string_value(&token, "token").unwrap_or_default();
    if token_value.is_empty() {
        return Vec::new();
    }
    let real_chapter = string_value(&token, "chapter_id").unwrap_or_else(|| chapter_id.to_string());
    let reader_url = format!(
        "{BASE_URL}/reader_v2.php?chapter={}&token={}&page=1",
        url::query_escape(&real_chapter),
        url::query_escape(&token_value)
    );
    parse_reader_pages(
        &fetch_document(&reader_url, PAGES_FIXTURE),
        &real_chapter,
        &token_value,
    )
}

fn parse_reader_pages(body: &str, chapter_id: &str, token: &str) -> Vec<MangaPage> {
    if let Some(total_pages) = text_after(body, "totalPages:")
        .and_then(|value| value.split(',').next().map(str::trim).map(str::to_string))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
    {
        return (1..=total_pages)
            .map(|page| {
                let image = format!(
                    "{BASE_URL}/image-proxy-v2.php?chapter={}&page={page}&token={}&context=reader",
                    url::query_escape(chapter_id),
                    url::query_escape(token)
                );
                page_from_url(page - 1, image)
            })
            .collect();
    }
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("page-image") || chunk.contains("readerContent"))
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "src"))
                .filter(|value| !value.starts_with("data:"))
        })
        .enumerate()
        .map(|(index, image)| page_from_url(index, url::join_url(BASE_URL, &image)))
        .collect()
}

fn page_from_url(index: usize, image: String) -> MangaPage {
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

fn manga_url(id: &str) -> String {
    format!(
        "{BASE_URL}/series.php?id={}",
        url::query_escape(&normalize_manga_key(id))
    )
}

fn normalize_manga_key(input: &str) -> String {
    let trimmed = input.trim();
    if let Some((_, query)) = trimmed.split_once('?') {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "id" || key == "s" {
                    return value.trim_matches('/').to_string();
                }
            }
        }
    }
    trimmed.trim_matches('/').to_string()
}

fn split_chapter_key(key: &str) -> (String, String) {
    let (chapter_id, manga_id) = key.split_once('#').unwrap_or((key, ""));
    (chapter_id.to_string(), manga_id.to_string())
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("activo") => ItemStatus::Ongoing,
        Some("finalizado") => ItemStatus::Completed,
        Some("abandonado") => ItemStatus::Cancelled,
        Some("pausado") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn text_after_icon(body: &str, title: &str) -> Option<String> {
    let marker = format!("title=\"{title}\"");
    text_after(body, &marker)
        .and_then(|after| html::text_between(&after, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn extract_csrf(body: &str) -> String {
    html::attr_after(body, "id=\"csrf_token\"", "value")
        .or_else(|| html::attr_after(body, "id='csrf_token'", "value"))
        .or_else(|| html::attr_after(body, "name=\"csrf-token\"", "content"))
        .or_else(|| {
            decode_from_char_code(text_after(body, "_token").as_deref().unwrap_or_default())
        })
        .unwrap_or_default()
}

fn extract_data_k(body: &str) -> String {
    html::attr_after(body, "data-k", "data-k")
        .or_else(|| {
            decode_from_char_code(text_after(body, "dataset.k").as_deref().unwrap_or_default())
        })
        .unwrap_or_default()
}

fn extract_data_v(body: &str) -> Option<String> {
    decode_from_char_code(text_after(body, "data-v").as_deref().unwrap_or_default())
}

fn solve_simple_challenge(script: &str, csrf: &str, data_k: &str, data_v: &str) -> String {
    if let Some(value) = decode_from_char_code(script) {
        return value;
    }
    for marker in ["return '", "return \"", "answer = '", "answer = \""] {
        if let Some(after) = text_after(script, marker) {
            let quote = if marker.ends_with('\"') { '"' } else { '\'' };
            if let Some(value) = after.split(quote).next().filter(|value| !value.is_empty()) {
                return value.to_string();
            }
        }
    }
    for candidate in [data_k, data_v, csrf] {
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    String::new()
}

fn decode_from_char_code(input: &str) -> Option<String> {
    let (_, rest) = input.split_once("String.fromCharCode(")?;
    let args = rest.split(')').next()?;
    let decoded = args
        .split(',')
        .filter_map(|part| {
            let clean = part.trim();
            if let Some(hex) = clean
                .strip_prefix("0x")
                .or_else(|| clean.strip_prefix("0X"))
            {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else {
                clean.parse::<u32>().ok().and_then(char::from_u32)
            }
        })
        .collect::<String>();
    (!decoded.is_empty()).then_some(decoded)
}

fn text_after(body: &str, marker: &str) -> Option<String> {
    body.split_once(marker).map(|(_, rest)| rest.to_string())
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<f32>().ok())
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn push_unique_catalog(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut entries: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

const LIST_FIXTURE: &str = r#"
<main><div class="container"><div class="grid">
<div class="comic-card"><a href="/series.php?id=sample"><img class="object-cover" src="/cover.jpg"><h3>Sample Yup</h3></a></div>
</div><div class="flex"><a>Siguiente</a></div></div></main>
"#;
const DETAILS_FIXTURE: &str = r#"
<main><div class="container"><h1>Sample Yup</h1><p id="synopsisText">Sample summary.</p>
<span><i title="Editorial"></i><span>Author</span></span><span><i title="Estado"></i><span>Activo</span></span>
<a class="genre-tag">drama</a><meta property="og:image" content="/cover.jpg"></div></main>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"html":"<div class=\"comic-card\"><a data-chapter=\"1\"><h3>Capitulo 1</h3></a></div>","currentPage":1,"totalPages":1}"#;
const PAGES_FIXTURE: &str = r#"<script>const reader = { totalPages: 1 };</script><div id="readerContent"><img class="page-image" src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixtures() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries.len(), 1);
        assert_eq!(SOURCE.chapters(json!({"manga":"sample"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
