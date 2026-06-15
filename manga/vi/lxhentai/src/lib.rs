use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: LxHentai = LxHentai;
const DEFAULT_BASE_URL: &str = "https://lxmanga.space";

struct LxHentai;

impl MangaSource for LxHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "-views"
        } else {
            "-updated_at"
        };
        Ok(parse_listing(&fetch_document(&base, &browse_url(&base, page, sort, Some("ongoing,completed,paused")), LIST_FIXTURE), &base, page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(&base, query).or_else(|| query.strip_prefix("id:").map(|slug| format!("/truyen/{}", slug.trim_matches('/')))) {
            return Ok(Paged { entries: vec![details_by_key(&base, &key)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter(filters, "sort").unwrap_or("-updated_at");
        let search_type = filter(filters, "searchType").unwrap_or("name");
        let mut pairs = vec![format!("sort={}", url::query_escape(sort)), format!("page={page}")];
        if !query.is_empty() {
            pairs.push(format!("filter[{search_type}]={}", url::query_escape(query)));
        }
        if let Some(status) = multi_filter(filters, "status").filter(|v| !v.is_empty()) {
            pairs.push(format!("filter[status]={}", url::query_escape(&status)));
        }
        if let Some(genres) = multi_filter(filters, "acceptGenres").filter(|v| !v.is_empty()) {
            pairs.push(format!("filter[accept_genres]={}", url::query_escape(&genres)));
        }
        if let Some(genres) = multi_filter(filters, "rejectGenres").filter(|v| !v.is_empty()) {
            pairs.push(format!("filter[reject_genres]={}", url::query_escape(&genres)));
        }
        Ok(parse_listing(&fetch_document(&base, &format!("{base}/tim-kiem?{}", pairs.join("&")), LIST_FIXTURE), &base, page))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(parse_chapters(&fetch_document(&base, &absolute_url(&base, &key), DETAILS_FIXTURE), &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/truyen/sample/1".into());
        let chapter_url = absolute_url(&base, &key);
        let body = fetch_document(&base, &chapter_url, PAGES_FIXTURE);
        let action_token = meta_content(&body, "action_token");
        let mut images = action_token.as_deref()
            .map(|token| decode_local_images(&body, token))
            .unwrap_or_default();
        if images.is_empty() {
            images = body.split("<img").skip(1).filter_map(image_attr).map(|image| absolute_url(&base, &image)).collect();
        }
        if images.is_empty() {
            return Ok(vec![manga::text_page("Reader requires WebView JavaScript token extraction that Manatan source API does not currently expose.")]);
        }
        let token = action_token.unwrap_or_default();
        Ok(images.into_iter().enumerate().map(|(index, image)| page(index, &image, &chapter_url, &base, &token)).collect())
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section("popular", "Popular", self.list(with_listing(&request, "popular"))?),
            home_section("latest", "Latest", self.list(with_listing(&request, "latest"))?),
        ])
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
        if let Some(key) = key_from_url(&base, input) {
            let is_manga = key.split('/').filter(|s| !s.is_empty()).count() == 2;
            return Ok(Some(UrlResolveResult {
                item: is_manga.then(|| details_by_key(&base, &key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn browse_url(base: &str, page: u64, sort: &str, status: Option<&str>) -> String {
    let mut pairs = vec![format!("sort={}", url::query_escape(sort)), format!("page={page}")];
    if let Some(status) = status {
        pairs.push(format!("filter[status]={}", url::query_escape(status)));
    }
    format!("{base}/tim-kiem?{}", pairs.join("&"))
}

fn parse_listing(body: &str, base: &str, page: u64) -> Paged<CatalogItem> {
    let entries = body.split("manga-vertical").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "text-ellipsis", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
        let key = normalize_key(base, &href);
        let title = html::text_between(chunk, "text-ellipsis", "</a>")
            .or_else(|| html::text_between(chunk, "<a", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
        Some(CatalogItem {
            key: key.clone(),
            title,
            cover: background_image(chunk).or_else(|| html::attr_after(chunk, "cover", "data-bg")).map(|image| absolute_url(base, &image)),
            url: Some(absolute_url(base, &key)),
            language: Some("vi".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        })
    }).fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains(&format!("data-page=\"{}\"", page + 1)) || body.contains("pagination"),
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(&fetch_document(base, &absolute_url(base, key), DETAILS_FIXTURE), base, key)
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    let alt = info_row(body, "Tên khác:");
    let summary = html::text_between(body, "Tóm tắt", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty());
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "font-semibold", "</span>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: background_image(body).or_else(|| html::attr_after(body, "cover", "data-bg")).map(|image| absolute_url(base, &image)),
        authors: info_links(body, "/tac-gia/"),
        tags: info_links(body, "/the-loai/"),
        description: match (alt, summary) {
            (Some(a), Some(s)) => Some(format!("Tên khác: {a}\n\n{s}")),
            (Some(a), None) => Some(format!("Tên khác: {a}")),
            (None, s) => s,
        },
        status: parse_status(&info_row(body, "Tình trạng:").unwrap_or_default()),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains("timeago") && chunk.contains("/truyen/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(base, &href);
        let title = html::text_between(chunk, "text-ellipsis", "</span>")
            .or_else(|| html::text_between(chunk, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "Chapter".into());
        Some(MangaChapter {
            key: key.clone(),
            title: Some(title),
            date_uploaded: html::attr_after(chunk, "timeago", "datetime").and_then(|value| parse_iso_date(&value)),
            url: Some(absolute_url(base, &key)),
            ..MangaChapter::default()
        })
    }).fold(Vec::new(), push_unique_chapter)
}

fn decode_local_images(body: &str, token: &str) -> Vec<String> {
    let payload = body.split("var _u").nth(1).and_then(|tail| tail.split('=').nth(1)).and_then(|tail| tail.split("];").next()).map(|v| format!("{}]", v.trim())).unwrap_or_default();
    let rows = encrypted_rows(&payload);
    data_indices(body).into_iter().filter_map(|idx| rows.get(idx)).map(|codes| decode_image_url(codes, token)).filter(|v| !v.is_empty()).collect()
}

fn encrypted_rows(input: &str) -> Vec<Vec<u32>> {
    let mut rows = Vec::new();
    for part in input.split('[').skip(2) {
        let Some(raw) = part.split(']').next() else { continue; };
        let codes: Vec<u32> = raw.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        if !codes.is_empty() { rows.push(codes); }
    }
    rows
}

fn data_indices(body: &str) -> Vec<usize> {
    body.split("data-idx=").skip(1).filter_map(|tail| {
        let quote = tail.chars().next()?;
        let rest = tail.get(1..)?;
        rest.split(quote).next()?.parse().ok()
    }).collect()
}

fn decode_image_url(codes: &[u32], token: &str) -> String {
    if token.is_empty() { return String::new(); }
    let keys: Vec<u32> = token.chars().map(|c| c as u32).collect();
    codes.iter().enumerate().filter_map(|(idx, code)| char::from_u32(code ^ keys[idx % keys.len()])).collect()
}

fn meta_content(body: &str, name: &str) -> Option<String> {
    body.split("<meta").skip(1).find(|chunk| chunk.contains(&format!("name=\"{name}\"")) || chunk.contains(&format!("name='{name}'"))).and_then(|chunk| html::attr(chunk, "content"))
}

fn info_row(body: &str, label: &str) -> Option<String> {
    body.find(label).map(|idx| html::strip_tags(&body[idx..].split("</div>").next().unwrap_or_default()).replace(label, "").trim().to_string()).filter(|v| !v.is_empty())
}

fn info_links(body: &str, href_marker: &str) -> Vec<String> {
    body.split("<a").skip(1).filter(|chunk| chunk.contains(href_marker)).map(html::strip_tags).filter(|v| !v.is_empty()).collect()
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("completed") { ItemStatus::Completed }
    else if lower.contains("đang tiến hành") { ItemStatus::Ongoing }
    else { ItemStatus::Unknown }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?.replace('/', "-");
    dates::parse_ymd(&date)
}

fn page(index: usize, image: &str, referer: &str, base: &str, token: &str) -> MangaPage {
    let mut headers = manga::image_headers(referer);
    headers.insert("Origin".into(), base.into());
    if !token.is_empty() { headers.insert("Token".into(), token.into()); }
    MangaPage {
        content: PageContent::Url { url: image.into(), context: Some(headers.clone()) },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn background_image(input: &str) -> Option<String> {
    let tail = input.split("background-image").nth(1)?;
    let raw = tail.split("url(").nth(1)?.split(')').next()?.trim_matches(['\'', '"', ' ']);
    (!raw.is_empty()).then(|| raw.to_string())
}

fn base_url(request: &Value) -> String {
    request.get("preferences")
        .and_then(|p| p.get("overrideBaseUrl"))
        .and_then(Value::as_str)
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .unwrap_or(DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn normalize_key(base: &str, value: &str) -> String {
    if value.starts_with("http") { value.trim_start_matches(base).trim_end_matches('/').to_string() }
    else { format!("/{}", value.trim_start_matches('/').trim_end_matches('/')) }
}

fn absolute_url(base: &str, value: &str) -> String {
    if value.starts_with("http") { value.into() } else { format!("{base}/{}", value.trim_start_matches('/')) }
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input.starts_with(base).then(|| normalize_key(base, input)).filter(|key| key.contains("/truyen/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn multi_filter(filters: &Value, id: &str) -> Option<String> {
    match filters.get(id) {
        Some(Value::Array(values)) => Some(values.iter().filter_map(Value::as_str).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(",")),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    value["page"] = json!(1);
    value["listingId"] = json!(listing);
    value
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection { id: id.into(), title: title.into(), style: Some(HomeSectionStyle::Cover), entries: page.entries, has_more: page.has_next_page, ..HomeSection::default() }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|seen| seen.key == item.key) { items.push(item); }
    items
}

const LIST_FIXTURE: &str = r#"<div class="manga-vertical"><div class="cover" style="background-image:url('/cover.jpg')"></div><a class="text-ellipsis" href="/truyen/sample">Sample</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<span class="grow text-lg ml-1 text-ellipsis font-semibold">Sample</span><div class="cover" style="background-image:url('/cover.jpg')"></div><div><span class="font-semibold">Tác giả:</span><a href="/tac-gia/author">Author</a></div><div><span class="font-semibold">Thể loại:</span><a href="/the-loai/adult">Adult</a></div><div><span class="font-semibold">Tình trạng:</span>đang tiến hành</div><p>Tóm tắt</p><p>Summary</p><ul class="overflow-y-auto"><a href="/truyen/sample/1"><span class="text-ellipsis">Chapter 1</span><span class="timeago" datetime="2024-01-01T00:00:00.000+07:00"></span></a></ul>"#;
const PAGES_FIXTURE: &str = r#"<meta name="action_token" content="a"><script>var _u = [[14,15,12]];</script><div id="image-container" data-idx="0"></div>"#;

export_manga_source!(SOURCE);
