use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manatoki = Manatoki;
const BASE_URL: &str = "https://manatoki552.net";

struct Manatoki;

impl MangaSource for Manatoki {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = page(&request);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/bbs/board.php?bo_table=cartoon&page={page}"),
            LIST_FIXTURE,
        )))
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = page(&request);
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}/bbs/board.php?bo_table=cartoon&stx={}&page={page}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/bbs/board.php?bo_table=cartoon&wr_id=1".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/bbs/board.php?bo_table=cartoon&wr_id=1".to_string());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/bbs/board.php?bo_table=cartoon&wr_id=2".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
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
        return format!("/{}", value[index + 1..].trim_start_matches('/'));
    }
    format!("/{}", value.trim_start_matches('/'))
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("list-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "in-lable", "</a>")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| html::text_between(chunk, "item-subject", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manatoki".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
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
        has_next_page: body.contains("pagination li active")
            || body.contains("pagination")
            || body.contains("pg_end"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/bbs/board.php?bo_table=cartoon&wr_id=1".to_string());
    let title = html::text_between(body, "view-content", "</")
        .and_then(|value| html::text_between(&value, "<b", "</b>").or(Some(value)))
        .or_else(|| html::text_between(body, "<h1", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manatoki".into()));
    let info = body
        .split("view-content")
        .map(html::strip_tags)
        .collect::<Vec<_>>()
        .join(" ");
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "view-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(body, "view-content", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: labeled_values(&info, "작가 :"),
        tags: labeled_values(&info, "분류 :"),
        status: ItemStatus::Unknown,
        url: Some(absolute_url(&key)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chunks = body
        .split("list-item")
        .skip(1)
        .filter(|chunk| chunk.contains("item-subject") || chunk.contains("wr-num"))
        .collect::<Vec<_>>();
    let total = chunks.len();
    let mut chapters = chunks
        .into_iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "item-subject", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let number = html::text_between(chunk, "wr-num", "</")
                .map(|value| html::strip_tags(&value))
                .and_then(|value| value.parse::<f32>().ok())
                .or(Some((total.saturating_sub(index)) as f32));
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: number.map(|num| format!("Chapter {num}")),
                chapter_number: number,
                date_uploaded: html::text_between(chunk, "wr-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(parse_date),
                url: Some(absolute_url(&key)),
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
    let decoded = find_between(body, "var manamoa_img", "'")
        .and_then(|value| value.split('\'').nth(1).map(ToString::to_string))
        .and_then(|value| STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let source = decoded.as_deref().unwrap_or(body);
    source
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("view-content") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn labeled_values(info: &str, label: &str) -> Vec<String> {
    info.split(label)
        .nth(1)
        .map(|value| value.split('•').next().unwrap_or(value))
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_date(input: String) -> Option<i64> {
    let parts = input
        .split('.')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    if parts.len() == 3 {
        Some(parts[0] * 10_000 + parts[1] * 100 + parts[2])
    } else {
        None
    }
}

fn find_between(input: &str, start: &str, end: &str) -> Option<String> {
    let rest = input.split(start).nth(1)?;
    Some(rest.split(end).next()?.to_string())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="list-row"><div class="list-item"><div class="img-item"><a href="https://manatoki552.net/bbs/board.php?bo_table=cartoon&wr_id=1" title="Sample Manatoki"><img src="/cover.jpg"></a></div><div class="in-lable"><a><font>Sample Manatoki</font></a></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="view-img"><img src="/cover.jpg"></div>
<div class="view-content"><span><b>Sample Manatoki</b></span></div>
<div class="view-content">작가 : Sample Author • 분류 : Action, Fantasy</div>
<div class="list-body"><div class="list-item"><a class="item-subject" href="/bbs/board.php?bo_table=cartoon&wr_id=2"></a><span class="wr-num">1</span><span class="wr-date">2024.01.01</span></div></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="view-content"><img src="/page1.jpg"></div>
"#;
