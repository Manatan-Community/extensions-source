use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HenChan = HenChan;
const DEFAULT_BASE_URL: &str = "https://xxl.hentaichan.live";

struct HenChan;

impl MangaSource for HenChan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{base}/manga/newest?offset={}", 20 * page.saturating_sub(1))
        } else {
            search_url(&base, page, "", request.get("filters").unwrap_or(&Value::Null))
        };
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("http://") || query.starts_with("https://") {
            let key = normalize_key(&base, query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = search_url(&base, page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_listing(&fetch_document(&base, &target, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), Some(key), &base))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let chapter_key = if key.contains("/manga/") && !is_exhen_key(&key) { key.replace("/manga/", "/related/") } else { key };
        let body = fetch_document(&base, &absolute_url(&base, &chapter_key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body, &base);
        if chapters.is_empty() {
            chapters.push(MangaChapter {
                key: chapter_key.replace("/related/", "/manga/"),
                title: Some("Chapter".into()),
                chapter_number: Some(1.0),
                url: Some(absolute_url(&base, &chapter_key.replace("/related/", "/manga/"))),
                ..MangaChapter::default()
            });
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample".into());
        let page_key = if key.contains("/manga/") { key.replace("/manga/", "/online/") } else { key };
        Ok(parse_pages(&fetch_document(&base, &absolute_url(&base, &page_key), PAGES_FIXTURE), &base))
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
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(&base, input, DETAILS_FIXTURE), Some(key), &base)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base.trim_end_matches('/')))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("domain")).and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn search_url(base: &str, page: u64, query: &str, filters: &Value) -> String {
    if !query.is_empty() {
        return format!("{base}/?do=search&subaction=search&story={}&search_start={page}", url::query_escape(query));
    }
    let mut genres = selected_values(filters.get("genres"));
    genres.extend(selected_values(filters.get("excludedGenres")).into_iter().map(|value| format!("-{value}")));
    let order = filter_id(filters, "sort").unwrap_or("favdesc");
    let offset = 20 * page.saturating_sub(1);
    if genres.is_empty() {
        match order {
            "dateasc" => format!("{base}/manga/new&n=dateasc?offset={offset}"),
            "abcasc" => format!("{base}/manga/new&n=abcasc?offset={offset}"),
            _ => format!("{base}/mostfavorites&sort=manga?offset={offset}"),
        }
    } else {
        let sort = match order {
            "dateasc" => "&n=dateasc",
            "abcasc" => "&n=abcasc",
            _ => "&n=favdesc",
        };
        format!("{base}/tags/{}&sort=manga{sort}?offset={offset}", genres.join("+"))
    }
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body.split("<div").skip(1)
        .filter(|chunk| chunk.contains("content_row") || chunk.contains("related") || chunk.contains("item"))
        .filter(|chunk| !chunk.contains("Тип"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !(href.contains("/manga/") || href.contains("/online/")) { return None; }
            let key = normalize_key(base, &href).replace("/online/", "/manga/");
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v)))
                    .or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HenChan".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|image| hq_thumbnail(base, &image)),
                url: Some(absolute_url(base, &key)),
                language: Some("ru".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { has_next_page: body.contains("pagination") || entries.len() >= 20, entries }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "title_top_a", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HenChan".into())),
        cover: html::attr_after(body, "id=\"cover\"", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|image| hq_thumbnail(base, &image)),
        description: html::text_between(body, "description", "</div>").or_else(|| html::text_between(body, "row4_right", "</div>")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()),
        tags: body.split("/tags/").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</a>").map(|v| html::strip_tags(&v))).filter(|v| !v.is_empty()).collect(),
        status: ItemStatus::Completed,
        url: Some(absolute_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    if body.contains("/manga/") && !body.contains("related") {
        return vec![MangaChapter {
            key: html::attr_after(body, "rel=\"canonical\"", "href").map(|v| normalize_key(base, &v)).unwrap_or_else(|| "/manga/sample".into()),
            title: html::text_between(body, "title_top_a", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).or(Some("Chapter".into())),
            chapter_number: Some(1.0),
            url: html::attr_after(body, "rel=\"canonical\"", "href").or_else(|| Some(format!("{base}/manga/sample"))),
            ..MangaChapter::default()
        }];
    }
    body.split("related").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(base, &href);
        Some(MangaChapter {
            key: key.clone(),
            title: html::attr_after(chunk, "<a", "title").or_else(|| html::text_between(chunk, "<h2", "</h2>").map(|v| html::strip_tags(&v))).or(Some("Chapter".into())),
            chapter_number: chapter_number(chunk),
            url: Some(absolute_url(base, &key)),
            ..MangaChapter::default()
        })
    }).collect::<Vec<_>>().into_iter().rev().collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("fullimg\": [").nth(1)
        .and_then(|v| v.split(']').next())
        .unwrap_or("")
        .replace(['"', '\''], "")
        .split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(base, &image), context: Some(manga::image_headers(base)) },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(base: &str, value: &str) -> String {
    let path = value.strip_prefix(base).unwrap_or(value).split('?').next().unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(base: &str, key: &str) -> String { url::join_url(base, key) }

fn hq_thumbnail(base: &str, image: &str) -> String {
    let host = base.trim_start_matches("https://").trim_start_matches("http://");
    absolute_url(base, image).replace("manganew_thumbs_blur", "showfull_retina/manga").replace("manganew_thumbs", "showfull_retina/manga").replace(&format!("_{host}"), "_hentaichan.ru")
}

fn is_exhen_key(key: &str) -> bool { key.contains("manganew_thumbs_blur") }

fn chapter_number(chunk: &str) -> Option<f32> {
    let text = html::strip_tags(chunk).to_lowercase();
    for marker in ["глава ", "часть "] {
        if let Some(num) = text.split(marker).nth(1).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<f32>().ok()) {
            return Some(num);
        }
    }
    None
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).filter_map(option_id).collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value))).filter(|value| !value.is_empty())
}

fn option_id(value: &str) -> Option<String> {
    let id = value.trim().split_once(':').map(|(id, _)| id).unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) { items.push(item); }
    items
}

const LIST_FIXTURE: &str = r#"<div class="content_row"><a href="/manga/sample" title="Sample"><img src="/manganew_thumbs/sample.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<a class="title_top_a">Sample</a><img id="cover" src="/manganew_thumbs/sample.jpg"><div class="related"><h2><a href="/manga/sample" title="Chapter">Chapter</a></h2></div>"#;
const PAGES_FIXTURE: &str = r#"<script>fullimg": ["https://static.hentaichan.live/page1.jpg", "https://static.hentaichan.live/page2.jpg"]</script>"#;

export_manga_source!(SOURCE);
