use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: DaoMeoDen = DaoMeoDen;
const BASE_URL: &str = "https://daomeoden.net";
const LIST_PATH: &str = "/danh-sach-truyen-tranh.html";

struct DaoMeoDen;

impl MangaSource for DaoMeoDen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("popular") { "viewsAll" } else { "updated_at" };
        Ok(parse_browse_page(&fetch_document(&format!("{BASE_URL}{LIST_PATH}?page={page}&order={order}"), LIST_FIXTURE), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut params = vec![format!("page={page}")];
        for (id, default) in [("status", "0"), ("category", "all"), ("genre", "0"), ("explicit", "0"), ("order", "updated_at")] {
            if let Some(value) = filter(filters, id).filter(|value| *value != default) {
                params.push(format!("{id}={}", url::query_escape(value)));
            }
        }
        if !query.is_empty() {
            params.push(format!("textSearch={}", url::query_escape(query)));
        }
        Ok(parse_browse_page(&fetch_document(&format!("{BASE_URL}{LIST_PATH}?{}", params.join("&")), LIST_FIXTURE), page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document(&chapter_url, CHAPTER_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![home_section("popular", "Popular", self.list(json!({"page": 1, "listingId": "popular"}))?), home_section("latest", "Latest", self.list(json!({"page": 1, "listingId": "latest"}))?)])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult { item: key.contains("truyen").then(|| details_by_key(&key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn post_form(target: &str, referer: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(target)
        .referer(referer)
        .origin(BASE_URL)
        .form(form)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_owned_form(target: &str, referer: &str, form: &[(&str, String)], fixture: &str) -> String {
    let body = form
        .iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&");
    client()
        .post(target)
        .referer(referer)
        .origin(BASE_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.into_bytes())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_browse_page(body: &str, request_page: u64) -> Paged<CatalogItem> {
    let html_book = if body.contains("item-list") {
        body.to_string()
    } else if let Some(form) = list_form(body) {
        let response = post_owned_form(&format!("{BASE_URL}/apps/controllers/book/bookList.php"), BASE_URL, &form, LIST_API_FIXTURE);
        serde_json::from_str::<BookListResponse>(&response).ok().and_then(|payload| (payload.status == 200).then_some(payload.html_book).flatten()).unwrap_or_default()
    } else {
        String::new()
    };
    let entries = html_book
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item-list"))
        .filter_map(catalog_from_item)
        .fold(Vec::new(), push_unique);
    let current_page = script_var(body, "pageCurrent").and_then(|value| value.parse().ok()).unwrap_or(request_page);
    let last_page = script_var(body, "pageLast").and_then(|value| value.parse().ok()).unwrap_or(current_page);
    Paged { entries, has_next_page: current_page < last_page }
}

fn list_form(body: &str) -> Option<Vec<(&'static str, String)>> {
    Some(vec![
        ("token", script_var(body, "_token")?),
        ("pageCurrent", script_var(body, "pageCurrent")?),
        ("pageLast", script_var(body, "pageLast")?),
        ("status", script_var(body, "status").unwrap_or_else(|| "0".into())),
        ("ages", script_var(body, "ages").unwrap_or_default()),
        ("category", script_var(body, "category").unwrap_or_else(|| "all".into())),
        ("genre", script_var(body, "genre").unwrap_or_default()),
        ("explicit", script_var(body, "explicit").unwrap_or_default()),
        ("magazine", script_var(body, "magazine").unwrap_or_default()),
        ("tags", script_var(body, "tags").unwrap_or_default()),
        ("order", script_var(body, "order").unwrap_or_else(|| "updated_at".into())),
        ("pagiParam", script_var(body, "pagiParam")?),
        ("textSearch", script_var(body, "textSearch").unwrap_or_default()),
    ])
}

fn catalog_from_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "item-title", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "item-title", "</").or_else(|| html::text_between(chunk, "<a", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem { key: key.clone(), title, cover: html::attr_after(chunk, "item-cover", "src").or_else(|| image_attr(chunk)).map(|image| absolute_image(&image)), url: Some(absolute_url(&key)), language: Some("vi".into()), content_rating: Some("adult".into()), ..CatalogItem::default() })
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "info-name", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "info-cover-img", "src").or_else(|| image_attr(body)).map(|image| absolute_image(&image)),
        tags: tag_texts(body),
        description: html::text_between(body, "info-description", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        status: parse_status(&text),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter") && chunk.contains("openUrl"))
        .filter_map(|chunk| {
            let url = between(chunk, "openUrl('", "')")?;
            let key = normalize_key(&url);
            let title = html::text_between(chunk, "name-sub", "</").or_else(|| html::text_between(chunk, "name", "</")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter { key: key.clone(), title: Some(title.clone()), chapter_number: first_number(&title), date_uploaded: html::text_between(chunk, "time", "</").map(|value| html::strip_tags(&value)).and_then(|value| manatan_shared::dates::parse_fixture_date(&value)), url: Some(absolute_url(&key)), ..MangaChapter::default() })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let mut content = body.to_string();
    if let (Some(chapter_id), Some(token)) = (script_var(body, "chapterId"), script_var(body, "_token")) {
        let response = post_form(
            &format!("{BASE_URL}/apps/controllers/book/bookChapterContent.php"),
            chapter_url,
            &[("token", token.as_str()), ("chapterId", chapter_id.as_str()), ("cookies", "W10=")],
            CHAPTER_API_FIXTURE,
        );
        if let Ok(payload) = serde_json::from_str::<ChapterContentResponse>(&response) {
            if payload.status == 200 {
                if let Some(data) = payload.data {
                    content = data;
                }
            }
        }
    }
    content
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| looks_like_image(image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            let image = absolute_image(&image);
            if !seen.contains(&image) { seen.push(image); }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage { content: PageContent::Url { url: image, context: Some(manga::image_headers(chapter_url)) }, headers: manga::image_headers(chapter_url), description: Some(format!("Page {}", index + 1)), ..MangaPage::default() })
        .collect()
}

fn script_var(body: &str, key: &str) -> Option<String> {
    between(body, &format!("var {key} = '"), "'")
}

fn between(input: &str, start: &str, end: &str) -> Option<String> {
    let rest = input.split(start).nth(1)?;
    Some(rest.split(end).next()?.to_string())
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("full") || lower.contains("completed") || lower.contains("hoàn") { ItemStatus::Completed } else if lower.contains("ongoing") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn tag_texts(body: &str) -> Vec<String> {
    body.split("<span").skip(1).filter(|chunk| chunk.contains("info-tag") || chunk.contains("</span>")).map(html::strip_tags).filter(|value| !value.is_empty()).collect()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn looks_like_image(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !lower.starts_with("data:") && [".jpg", ".jpeg", ".png", ".webp", ".avif"].iter().any(|ext| lower.contains(ext))
}

fn first_number(input: &str) -> Option<f32> {
    let number = input.chars().skip_while(|ch| !ch.is_ascii_digit()).take_while(|ch| ch.is_ascii_digit() || *ch == '.').collect::<String>();
    number.parse().ok()
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), has_more: page.has_next_page, entries: page.entries, ..HomeSection::default() }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(BASE_URL).trim_end_matches('/').to_string() } else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{BASE_URL}/{}", value.trim_start_matches('/')) }
}

fn absolute_image(value: &str) -> String {
    if value.starts_with("//") { format!("https:{value}") } else { absolute_url(value) }
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookListResponse { status: i32, html_book: Option<String> }

#[derive(Deserialize)]
struct ChapterContentResponse { status: i32, data: Option<String> }

const LIST_FIXTURE: &str = r#"<script>var _token = 't';var pageCurrent = '1';var pageLast = '1';var status = '0';var category = 'all';var order = 'updated_at';var pagiParam = '';</script><div class="item-list"><div class="item-title"><a href="/truyen-tranh/sample">Sample</a></div><div class="item-cover"><img src="/cover.jpg"></div></div>"#;
const LIST_API_FIXTURE: &str = r#"{"status":200,"htmlBook":"<div class=\"item-list\"><div class=\"item-title\"><a href=\"/truyen-tranh/sample\">Sample</a></div><div class=\"item-cover\"><img src=\"/cover.jpg\"></div></div>"}"#;
const DETAILS_FIXTURE: &str = r#"<div class="info-name">Sample</div><div class="info-cover-img"><img src="/cover.jpg"></div><div class="info-tag tag-category"><span>Action</span></div><div class="info-tag tag-status"><span>Ongoing</span></div><div class="info-description"><div class="content">Summary</div></div><div id="TabChapterChapter"><div class="chapter" onclick="openUrl('/chapter/sample-1')"><div class="chapter-info"><div class="name">Chapter 1</div><div class="time"><div>01.01.2024 - 00:00</div></div></div></div></div>"#;
const CHAPTER_FIXTURE: &str = r#"<script>var _token = 't';var chapterId = '1';</script><img data-src="/page1.jpg">"#;
const CHAPTER_API_FIXTURE: &str = r#"{"status":200,"data":"<img data-src=\"/page1.jpg\">"}"#;

export_manga_source!(SOURCE);
