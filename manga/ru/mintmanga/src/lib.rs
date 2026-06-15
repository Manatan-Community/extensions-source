use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MintManga = MintManga;
const DEFAULT_BASE_URL: &str = "https://2.mintmanga.one";
const NAME: &str = "MintManga";
const NEED_AUTH: bool = true;
const LIST_FIXTURE: &str = r#"<div class="tile"><img class="lazy" data-original="/cover_p.jpg"><h3><a href="/sample" title="Sample">Sample</a></h3></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="cr-hero-names__main">Sample</div><div class="cr-info-details"><div><span class="cr-info-details-item__title">Выпуск</span><span class="cr-info-details-item__status">продолжается</span></div></div><div class="cr-main-person-item"><span class="cr-main-person-item__role">Автор</span><span class="cr-main-person-item__name">Author</span></div><a href="/list/category/el_9451">Манга</a><a href="/list/limitation/el_6181">PG-13</a><div class="cr-tags"><span class="cr-tags__item"><span>боевик</span></span></div><div class="cr-description__content">Description</div><img class="cr-hero-poster__img" src="/cover.jpg"><table><tr class="item-row"><td class="item-title" data-num="10"></td><td><a class="chapter-link" href="/sample/vol1/1" title="Team">Глава 1</a></td><td class="d-none">01.01.24</td></tr></table><script>rm_h.readerInit('1','//img.example/','/p.jpg');</script>"#;

struct MintManga;

impl MangaSource for MintManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "updated" } else { "rate" };
        Ok(parse_listing(&fetch_document(&request, &format!("{base}/list?sortType={sort}&offset={}", 50 * page.saturating_sub(1)), LIST_FIXTURE), &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(&base) || query.starts_with("slug:") {
            let key = normalize_key(&base, query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document(&request, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = search_url(&base, page, query, request.get("filters"));
        Ok(parse_listing(&fetch_document(&request, &target, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(&fetch_document(&request, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&request, &absolute_url(&base, &key), DETAILS_FIXTURE);
        if NEED_AUTH && !body.contains("user-avatar") {
            return Ok(Vec::new());
        }
        Ok(parse_chapters(&body, &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/vol1/1?mtr=true".into());
        let body = fetch_document(&request, &absolute_url(&base, &key), DETAILS_FIXTURE);
        if NEED_AUTH && !body.contains("user-avatar") {
            return Ok(Vec::new());
        }
        Ok(parse_pages(&body, &base))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base = base_url(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(&base) {
            let key = normalize_key(&base, input);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_document(&request, input, DETAILS_FIXTURE), Some(key), &base)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client(request: &Value) -> HttpClient {
    let base = base_url(request);
    HttpClient::browser()
        .with_header("User-Agent", preference(request, "userAgent", "arora"))
        .with_referer(base.clone())
        .with_cookies_for(&base)
        .with_webview_challenge_fallback()
}

fn fetch_document(request: &Value, target: &str, fixture: &str) -> String {
    client(request).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    preference(request, "domain", DEFAULT_BASE_URL).trim_end_matches('/').to_string()
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request.get("preferences").and_then(|p| p.get(key)).and_then(Value::as_str).filter(|v| !v.is_empty()).unwrap_or(default).to_string()
}

fn search_url(base: &str, page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = vec![format!("offset={}", 50 * page.saturating_sub(1))];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    if let Some(sort) = filter_id(filters, "sortType").filter(|v| !v.is_empty()) {
        params.push(format!("sortType={sort}"));
    }
    for group in ["category", "genres", "age", "more", "additional"] {
        for value in selected_values(filters.and_then(|f| f.get(group))) {
            params.push(format!("{}==in", url::query_escape(&value)));
        }
    }
    format!("{base}/search/advancedResults?{}", params.join("&"))
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body.split("div class=\"tile").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(base, &href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::attr_after(chunk, "<a", "title").or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
            cover: html::attr_after(chunk, "img", "data-original").or_else(|| html::attr_after(chunk, "img", "src")).map(|v| absolute_url(base, &v.replace("_p.", "."))),
            url: Some(absolute_url(base, &key)),
            language: Some("ru".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { has_next_page: body.contains("nextLink"), entries }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    let release = detail_value(body, "Выпуск").unwrap_or_default();
    let translation = detail_value(body, "Перевод").unwrap_or_default();
    let mut tags = Vec::new();
    tags.extend(body.split("<a").skip(1).filter(|c| c.contains("/list/category/") || c.contains("/list/limitation/")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))));
    tags.extend(body.split("cr-tags__item").skip(1).filter_map(|c| html::text_between(c, "<span", "</span>").map(|v| html::strip_tags(&v))));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "cr-hero-names__main", "</").or_else(|| html::attr_after(body, "itemprop=name", "content")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
        cover: html::attr_after(body, "cr-hero-poster__img", "src").or_else(|| html::attr_after(body, "cr-hero-overlay__bg", "data-bg")).map(|v| absolute_url(base, &v)),
        authors: people(body, "автор"),
        artists: people(body, "худож"),
        tags,
        description: description(body),
        status: parse_status(&release, &translation),
        url: Some(absolute_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("item-row").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "chapter-link", "href")?;
        let key = normalize_key(base, &href);
        let number = html::attr_after(chunk, "item-title", "data-num").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 10.0);
        Some(MangaChapter {
            key: format!("{key}?mtr=true"),
            title: html::text_between(chunk, "chapter-link", "</a>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
            scanlators: html::attr_after(chunk, "chapter-link", "title").filter(|v| !v.is_empty()).into_iter().collect(),
            chapter_number: number,
            date_uploaded: html::text_between(chunk, "d-none", "</").map(|v| html::strip_tags(&v)).and_then(|v| dates::parse_fixture_date(&v)),
            url: Some(absolute_url(base, &key)),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    let marker = if body.contains("rm_h.readerInit(") { "rm_h.readerInit(" } else { "rm_h.readerDoInit(" };
    let script = body.split(marker).nth(1).and_then(|v| v.split(");").next()).unwrap_or_default();
    let fields = script.split('\'').skip(1).step_by(2).collect::<Vec<_>>();
    fields.chunks(3).enumerate().filter_map(|(index, parts)| {
        if parts.len() < 3 { return None; }
        let mut image = if parts[1].is_empty() && parts[2].starts_with("/static/") {
            format!("{base}{}", parts[2])
        } else if parts[1].ends_with("/manga/") {
            format!("{}{}", parts[0], parts[2])
        } else {
            format!("{}{}{}", parts[1], parts[0], parts[2])
        };
        if !image.contains("://") { image = format!("https:{image}"); }
        Some(MangaPage {
            content: PageContent::Url { url: image.replace("//resh", "//h"), context: None },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
    }).collect()
}

fn description(body: &str) -> Option<String> {
    let rating = html::text_between(body, "cr-hero-rating__value", "</").map(|v| html::strip_tags(&v));
    let votes = html::text_between(body, "cr-hero-rating__text", "</").map(|v| html::strip_tags(&v));
    let prefix = rating.map(|r| format!("Рейтинг: {r}{}\n", votes.map(|v| format!(" ({v})")).unwrap_or_default())).unwrap_or_default();
    let text = html::text_between(body, "cr-description__content", "</").map(|v| html::strip_tags(&v)).unwrap_or_default();
    let out = format!("{prefix}{text}");
    (!out.trim().is_empty()).then(|| out)
}

fn detail_value(body: &str, label: &str) -> Option<String> {
    body.split("cr-info-details-item__title").skip(1).find(|c| c.contains(label)).and_then(|c| html::text_between(c, "cr-info-details-item__status", "</")).map(|v| html::strip_tags(&v).to_lowercase())
}

fn people(body: &str, role: &str) -> Vec<String> {
    body.split("cr-main-person-item").skip(1).filter(|c| c.to_lowercase().contains(role)).filter_map(|c| html::text_between(c, "cr-main-person-item__name", "</").map(|v| html::strip_tags(&v))).collect()
}

fn parse_status(release: &str, translation: &str) -> ItemStatus {
    if release.contains("заверш") && translation.contains("заверш") { ItemStatus::Completed } else if release.contains("заверш") { ItemStatus::Completed } else if release.contains("приост") || release.contains("заморож") { ItemStatus::Hiatus } else if release.contains("продолж") || release.contains("начат") { ItemStatus::Ongoing } else { ItemStatus::Unknown }
}

fn normalize_key(base: &str, value: &str) -> String {
    let path = value.trim_start_matches("slug:").trim_start_matches(base);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(base: &str, value: &str) -> String {
    url::join_url(base, value)
}

fn filter_id<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters.and_then(|f| f.get(id)).and_then(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)))
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).map(ToString::to_string).collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(Value::Object(object)) => object.values().filter_map(Value::as_str).map(ToString::to_string).collect(),
        _ => Vec::new(),
    }
}

export_manga_source!(SOURCE);
