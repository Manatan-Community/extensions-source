use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Nudemoon = Nudemoon;
const DEFAULT_BASE_URL: &str = "https://nude-moon.org";
const LIST_FIXTURE: &str = r#"<table class="news_pic2"><a href="/sample"><img src="/cover.jpg"><h2>Sample</h2></a></table>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><meta property="og:image" content="/cover.jpg"><table class="news_pic2"><a href="/mangaka/a">Author</a><div class="tag-links"><a>романтика</a></div><span class="small2">1 Января 2024</span></table><div class="description">Description</div><td class="button"><a href="/sample/all">Все главы</a></td><img title="p" loading="lazy" data-src="/page.jpg">"#;

struct Nudemoon;

impl MangaSource for Nudemoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "date" } else { "views" };
        Ok(parse_listing(&fetch_document(&base, &format!("{base}/all_manga?{order}&rowstart={}", 30 * page.saturating_sub(1)), LIST_FIXTURE), &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(&base) || query.starts_with("slug:") {
            let key = normalize_key(&base, query);
            return Ok(Paged { entries: vec![parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() { catalog_url(&base, page, request.get("filters")) } else { format!("{base}/search?stext={}&rowstart={}", url::query_escape(query), 30 * page.saturating_sub(1)) };
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        let body = fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE);
        let Some(all) = html::attr_after(&body, "Все главы", "href") else {
            return Ok(vec![single_chapter(&body, &key, &base)]);
        };
        let mut link = absolute_url(&base, &all);
        let mut out = Vec::new();
        loop {
            let page = fetch_document(&base, &link, DETAILS_FIXTURE);
            let chapters = parse_chapter_page(&page, &base);
            if chapters.is_empty() {
                out.push(single_chapter(&body, &key, &base));
                break;
            }
            out.extend(chapters);
            if let Some(next) = html::attr_after(&page, "a class=\"small", "href").filter(|_| page.contains("&gt;") || page.contains(">")) {
                link = absolute_url(&base, &next);
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".into());
        Ok(parse_pages(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), &base))
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
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch_document(&base, input, DETAILS_FIXTURE), Some(key), &base)), url: Some(input.to_string()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }), url: Some(input.to_string()), ..UrlResolveResult::default() }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_header("Cookie", "NMfYa=1; nm_mobile=1;")
        .with_header("User-Agent", "Mozilla/5.0 (Linux; Android 10) AppleWebKit/537.36 Chrome/100.0 Mobile Safari/537.36")
        .with_referer(base.to_string())
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("domain")).and_then(Value::as_str)
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn catalog_url(base: &str, page: u64, filters: Option<&Value>) -> String {
    let tags = selected_values(filters.and_then(|f| f.get("tags")));
    let order = filter_id(filters, "order").unwrap_or("views");
    let start = 30 * page.saturating_sub(1);
    if tags.is_empty() {
        format!("{base}/all_manga?{order}&rowstart={start}")
    } else {
        format!("{base}/tags/{}&{order}&rowstart={start}", tags.join("+"))
    }
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body.split("news_pic2").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "a", "href")?;
        let key = normalize_key(base, &href);
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v).split(" / ").next().unwrap_or("").split(" №").next().unwrap_or("").trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Nude-Moon".into())),
            cover: html::attr_after(chunk, "<img", "data-src").or_else(|| html::attr_after(chunk, "<img", "src")).map(|v| absolute_url(base, &v)),
            url: Some(absolute_url(base, &key)),
            language: Some("ru".into()),
            content_rating: Some("adult".into()),
            initialized: false,
            ..CatalogItem::default()
        })
    }).collect::<Vec<_>>();
    Paged { has_next_page: body.contains("a class=\"small") && body.contains("&gt;"), entries }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v).split(" / ").next().unwrap_or("").split(" №").next().unwrap_or("").trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Nude-Moon".into())),
        cover: html::attr_after(body, "property=\"og:image\"", "content").or_else(|| html::attr_after(body, "<img", "src")).map(|v| absolute_url(base, &v)),
        authors: body.split("<a").skip(1).filter(|c| c.contains("mangaka")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).collect(),
        tags: body.split("<a").skip(1).filter(|c| c.contains("tag") || c.contains("tags")).filter_map(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).collect(),
        description: html::text_between(body, "description", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_page(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("news_pic2").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(base, &href);
        let name = html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty())?;
        Some(MangaChapter {
            key: key.clone(),
            title: Some(name.clone()),
            scanlators: chunk.split("<a").skip(1).find(|c| c.contains("perevod")).and_then(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).into_iter().collect(),
            chapter_number: name.split('№').nth(1).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok()),
            date_uploaded: parse_date(&html::strip_tags(chunk)),
            url: Some(absolute_url(base, &key)),
            ..MangaChapter::default()
        })
    }).collect()
}

fn single_chapter(body: &str, key: &str, base: &str) -> MangaChapter {
    let title = html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)).unwrap_or_else(|| "Сингл".into());
    MangaChapter {
        key: key.to_string(),
        title: Some(format!("{title} Сингл")),
        scanlators: body.split("<a").skip(1).find(|c| c.contains("perevod")).and_then(|c| html::text_between(c, ">", "</a>").map(|v| html::strip_tags(&v))).into_iter().collect(),
        chapter_number: Some(0.0),
        date_uploaded: parse_date(&html::strip_tags(body)),
        url: Some(absolute_url(base, key)),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img").skip(1).filter(|c| c.contains("loading=\"lazy\"") || c.contains("title=")).filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))).enumerate().map(|(i, image)| MangaPage {
        content: PageContent::Url { url: absolute_url(base, &image), context: None },
        headers: manga::image_headers(base),
        description: Some(format!("Page {}", i + 1)),
        ..MangaPage::default()
    }).collect()
}

fn parse_date(text: &str) -> Option<i64> {
    let normalized = text.replace("Января", "January").replace("Февраля", "February").replace("Марта", "March").replace("Апреля", "April").replace("Мая", "May").replace("Июня", "June").replace("Июля", "July").replace("Августа", "August").replace("Сентября", "September").replace("Октября", "October").replace("Ноября", "November").replace("Декабря", "December");
    normalized.split("  ").find_map(dates::parse_fixture_date)
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
