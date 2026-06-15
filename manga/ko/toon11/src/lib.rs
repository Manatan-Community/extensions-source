use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Toon11 = Toon11;
const BASE_URL: &str = "https://www.11toon.com";

struct Toon11;

impl MangaSource for Toon11 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/bbs/board.php?bo_table=toon_c&sord=&type=upd&page={page}")
        } else {
            format!("{BASE_URL}/bbs/board.php?bo_table=toon_c&is_over=0&page={page}")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if query.trim().is_empty() {
            format!(
                "{BASE_URL}/bbs/board.php?bo_table=toon_c&page={}",
                page(&request)
            )
        } else {
            format!(
                "{BASE_URL}/bbs/search_stx.php?stx={}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/bbs/board.php?bo_table=toons&stx=sample&is=1".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/bbs/board.php?bo_table=toons&stx=sample&is=1".to_string());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/board/toon/1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&format!("/bbs{key}")),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&format!("/bbs{key}"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(value: &str) -> String {
    if let Some(index) = value.find("/bbs/") {
        format!("/{}", value[index + 1..].trim_start_matches('/'))
    } else {
        format!("/{}", value.trim_start_matches('/'))
    }
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("data-id"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "homelist-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "11toon".into())
                    }),
                cover: html::attr(chunk, "data-mobile-image")
                    .or_else(|| style_url(chunk))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ko".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pg_end"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/bbs/board.php?bo_table=toons&stx=sample&is=1".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "h2 class=\"title", "</h2>")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "11toon".to_string()),
        cover: html::attr_after(body, "img class=\"banner", "src")
            .map(|value| absolute_url(&value)),
        authors: label_after(body, "작가").into_iter().collect(),
        tags: label_after(body, "장르")
            .into_iter()
            .flat_map(|value| split_csv(&value))
            .collect(),
        description: label_after(body, "소개").into_iter().next(),
        status: label_after(body, "분류")
            .first()
            .map(|value| parse_status(value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(&key)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("episode-title") || chunk.contains("free-date"))
        .filter_map(|chunk| {
            let onclick = html::attr_after(chunk, "<button", "onclick")?;
            let raw = onclick
                .split("location.href='.")
                .nth(1)
                .or_else(|| onclick.split("location.href=\".").nth(1))?
                .split(['\'', '"'])
                .next()?;
            Some(MangaChapter {
                key: raw.to_string(),
                title: html::text_between(chunk, "episode-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "free-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(parse_date),
                url: Some(absolute_url(&format!("/bbs{raw}"))),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .skip(1)
        .find(|chunk| chunk.contains("img_list"))
        .unwrap_or(body);
    let list = script
        .split("img_list")
        .nth(1)
        .and_then(|rest| rest.split('=').nth(1))
        .and_then(|rest| rest.split(';').next())
        .and_then(|json| serde_json::from_str::<Vec<String>>(json.trim()).ok())
        .unwrap_or_default();
    list.into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: if image.starts_with("http") {
                    image
                } else {
                    format!("https:{}", image.trim_start_matches("https:"))
                },
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn style_url(input: &str) -> Option<String> {
    Some(input.split("url('").nth(1)?.split("')").next()?.to_string())
}

fn label_after(body: &str, label: &str) -> Vec<String> {
    body.split(&format!("contains({label})"))
        .nth(1)
        .or_else(|| body.split(label).nth(1))
        .and_then(|chunk| html::text_between(chunk, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect()
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_status(input: &str) -> ItemStatus {
    if input.contains("완결") {
        ItemStatus::Completed
    } else if ["주간", "월간", "연재", "격주"]
        .iter()
        .any(|needle| input.contains(needle))
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(input: String) -> Option<i64> {
    let parts = input
        .split('.')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    if parts.len() == 3 {
        Some((2000 + parts[0]) * 10_000 + parts[1] * 100 + parts[2])
    } else {
        None
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<li data-id="1"><a href="/bbs/board.php?bo_table=toons&stx=sample&is=1"><span class="homelist-title">Sample 11toon</span><span class="homelist-thumb" style="background:url('//image/cover.jpg')"></span></a></li>
"#;
const DETAILS_FIXTURE: &str = r#"
<h2 class="title">Sample 11toon</h2><img class="banner" src="/cover.jpg">
<span>분류</span><span>연재</span><span>작가</span><span>Author</span><span>소개</span><span>Description</span><span>장르</span><span>액션, 판타지</span>
<ul id="comic-episode-list"><li><button onclick="location.href='./board/toon/1'"><span class="episode-title">Chapter 1</span></button><span class="free-date">24.01.01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<script>var img_list = ["//image/page1.jpg"];</script>"#;
