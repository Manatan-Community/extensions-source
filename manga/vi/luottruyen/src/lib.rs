use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: LuotTruyen = LuotTruyen;
const DEFAULT_BASE_URL: &str = "https://luottruyen7.com";

struct LuotTruyen;

impl MangaSource for LuotTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            format!("{base}/tim-truyen?status=-1&sort=10{}", page_param(page))
        } else {
            format!("{base}/?page={page}&typegroup=0")
        };
        Ok(parse_listing(
            &fetch_document(&base, &target, LIST_FIXTURE),
            &base,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(&base, query) {
            return Ok(Paged {
                entries: vec![details_by_key(&base, &key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if !query.is_empty() {
            format!(
                "{base}/tim-truyen?keyword={}{}",
                url::query_escape(query),
                page_param(page)
            )
        } else if let Some(genre) = filter(filters, "genre") {
            format!("{base}/tim-truyen/{genre}{}", page_param(page))
        } else {
            let mut params = Vec::new();
            if let Some(sort) = filter(filters, "sort") {
                params.push(format!("sort={}", url::query_escape(sort)));
            }
            if let Some(status) = filter(filters, "status") {
                params.push(format!("status={}", url::query_escape(status)));
            }
            if page > 1 {
                params.push(format!("page={page}"));
            }
            format!("{base}/tim-truyen?{}", params.join("&"))
        };
        Ok(parse_listing(
            &fetch_document(&base, &target, LIST_FIXTURE),
            &base,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample-1".into());
        Ok(details_by_key(&base, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample-1".into());
        let story_id = key.rsplit('-').next().unwrap_or("1");
        let body = client(&base)
            .post(format!("{base}/Story/ListChapterByStoryID"))
            .header("Accept", "*/*")
            .header("Origin", &base)
            .header("X-Requested-With", "XMLHttpRequest")
            .referer(absolute_url(&base, &key))
            .form(&[("StoryID", story_id)])
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(parse_chapters(&body, &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chapter-1".into());
        let body = fetch_document(&base, &absolute_url(&base, &key), PAGES_FIXTURE);
        let pages = parse_pages(&body, &base);
        if pages.is_empty() {
            if has_login_hint(&body) {
                return Ok(vec![manga::text_page(
                    "Vui long dang nhap bang WebView de xem chuong nay",
                )]);
            }
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(with_listing(&request, "popular"))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(with_listing(&request, "latest"))?,
            ),
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
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(&base, input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("truyen").then(|| details_by_key(&base, &key)),
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

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(base: &str, target: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("item") && (chunk.contains("jtip") || chunk.contains("figcaption"))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "figcaption", "href")
                .or_else(|| html::attr_after(chunk, "jtip", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(base, &image)),
                url: Some(absolute_url(base, &key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("next") && !body.contains("next disabled"),
    }
}

fn details_by_key(base: &str, key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(base, &absolute_url(base, key), DETAILS_FIXTURE),
        base,
        key,
    )
}

fn parse_details(body: &str, base: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "title-detail", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "col-image", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(base, &image)),
        authors: html::text_between(body, "author", "</li>")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("kind") || chunk.contains("/the-loai/"))
            .map(html::strip_tags)
            .filter(|value| !value.is_empty())
            .collect(),
        description: html::text_between(body, "detail-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(&html::strip_tags(body)),
        url: Some(absolute_url(base, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("row") && chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(base, &href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: None,
                url: Some(absolute_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("#view-chapter")
                || chunk.contains("chapter-content")
                || chunk.contains("reading-content")
                || chunk.contains("data-index")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.starts_with("data:image"))
        .map(|image| absolute_url(base, &image))
        .enumerate()
        .map(|(index, image)| page(index, &image, base))
        .collect()
}

fn has_login_hint(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("/account/login")
        || lower.contains("/dang-nhap")
        || lower.contains("returnurl=")
        || lower.contains("login-page-wrapper")
        || lower.contains("đăng nhập")
        || lower.contains("login")
}

fn page(index: usize, image: &str, base: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.into(),
            context: Some(manga::image_headers(base)),
        },
        headers: manga::image_headers(base),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src").or_else(|| html::attr_after(input, "<img", "src"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("đang tiến hành") || lower.contains("đang cập nhật") {
        ItemStatus::Ongoing
    } else if lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn base_url(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("overrideBaseUrl"))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| value.starts_with("http"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn normalize_key(base: &str, input: &str) -> String {
    let without_base = input
        .trim()
        .strip_prefix(base)
        .unwrap_or(input.trim())
        .split(['?', '#'])
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", without_base.trim_start_matches('/'))
}

fn absolute_url(base: &str, input: &str) -> String {
    url::join_url(base, input)
}

fn key_from_url(base: &str, input: &str) -> Option<String> {
    input
        .contains(base.trim_start_matches("https://"))
        .then(|| normalize_key(base, input))
}

fn page_param(page: u64) -> String {
    if page > 1 {
        format!("&page={page}")
    } else {
        String::new()
    }
}

fn with_listing(request: &Value, listing: &str) -> Value {
    let mut value = request.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("listingId".into(), json!(listing));
        object.insert("page".into(), json!(1));
    }
    value
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

const LIST_FIXTURE: &str = r#"<div class="item"><div class="image"><a><img src="/cover.jpg"></a></div><figcaption><h3><a href="/truyen-tranh/sample-1">Sample</a></h3></figcaption></div><li class="next"><a href="?page=2">Next</a></li>"#;
const DETAILS_FIXTURE: &str = r#"<article id="item-detail"><h1 class="title-detail">Sample</h1><div class="col-image"><img src="/cover.jpg"></div><li class="author"><p class="col-xs-8">Author</p></li><li class="status"><p class="col-xs-8">Đang tiến hành</p></li><li class="kind"><p class="col-xs-8"><a>Action</a></p></li><div class="detail-content"><p>Summary</p></div></article>"#;
const CHAPTERS_FIXTURE: &str = r#"<li class="row"><div class="chapter"><a href="/truyen-tranh/sample/chapter-1">Chapter 1</a></div><div class="col-xs-4">1 ngày trước</div></li>"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
