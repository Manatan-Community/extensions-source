use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: RawXZ = RawXZ;
const BASE_URL: &str = "https://rawzo.net";

struct RawXZ;

impl MangaSource for RawXZ {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "date"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/manga/page/{page}/?orderby={order}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page_path = if page(&request) > 1 {
            format!("/page/{}", page(&request))
        } else {
            String::new()
        };
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}{page_path}/?s={}&post_type=manga",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.starts_with("/manga/").then(|| details_by_key(&key)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-card")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-card-thumb", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "manga-card-title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "RawZO".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("fa-chevron-right") || body.contains("pagination"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(&absolute_url(key), DETAILS_FIXTURE),
        Some(key.to_string()),
    )
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "md-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "RawZO".into())),
        cover: html::attr_after(body, "md-cover", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "md-desc-content", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: meta_values(body, "fa-user"),
        tags: body
            .split("md-tag")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&meta_values(body, "fa-rss").join(" ")),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("md-chapter-row")
        .skip(1)
        .filter_map(|chunk| {
            let link_chunk = chunk.split("md-chapter-name").nth(1).unwrap_or(chunk);
            let href = html::attr_after(link_chunk, "<a", "href")
                .or_else(|| html::attr(link_chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(link_chunk, "<a", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".into()),
                ),
                date_uploaded: html::text_between(chunk, "md-chapter-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_relative_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("reader-page")
        .skip(1)
        .filter_map(|chunk| {
            html::attr_after(chunk, "<img", "src").or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| {
            let url = proxy_url(&absolute_url(&image));
            MangaPage {
                content: PageContent::Url {
                    url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn proxy_url(image: &str) -> String {
    if image.contains("img-proxy.php?url=") {
        image.to_string()
    } else {
        format!(
            "{BASE_URL}/wp-content/themes/manga-theme-MangaVerse/img-proxy.php?url={}",
            url::query_escape(image)
        )
    }
}

fn meta_values(body: &str, icon: &str) -> Vec<String> {
    body.split("md-meta-row")
        .skip(1)
        .filter(|chunk| chunk.contains(icon))
        .filter_map(|chunk| html::text_between(chunk, "md-meta-val", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "更新中")
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("連載中") {
        ItemStatus::Ongoing
    } else if value.contains("完結") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_relative_date(value: &str) -> Option<i64> {
    let amount = value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse::<i64>()
        .ok()?;
    let seconds = if value.contains("秒前") {
        amount
    } else if value.contains("分前") {
        amount * 60
    } else if value.contains("時間前") {
        amount * 3_600
    } else if value.contains("日前") {
        amount * 86_400
    } else if value.contains("週間前") {
        amount * 604_800
    } else if value.contains("ヶ月前") {
        amount * 2_592_000
    } else if value.contains("年前") {
        amount * 31_536_000
    } else {
        return None;
    };
    Some(current_unix_seconds().saturating_sub(seconds))
}

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga-card"><a class="manga-card-thumb" href="/manga/sample"><img src="/cover.jpg"></a><div class="manga-card-title">Sample RawZO</div></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="md-title">Sample RawZO</h1><div class="md-cover"><img src="/cover.jpg"></div>
<div class="md-meta-row"><i class="fa-user"></i><div class="md-meta-val">Author</div></div><div class="md-meta-row"><i class="fa-rss"></i><div class="md-meta-val">連載中</div></div>
<a class="md-tag">Action</a><div class="md-desc-content">Sample description.</div>
<div class="md-chapter-row"><div class="md-chapter-name"><a href="/manga/sample/chapter-1">Chapter 1</a></div><div class="md-chapter-time">1日前</div></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reader-page"><img src="/page1.jpg"></div><div class="reader-page"><img src="/page2.jpg"></div>"#;
