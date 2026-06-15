use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: WolfDotCom = WolfDotCom;
const DEFAULT_DOMAIN_NUMBER: &str = "393";

struct WolfDotCom;

impl MangaSource for WolfDotCom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            "f"
        } else {
            "n"
        };
        let body = fetch_document(
            &format!("{base}/{}?o={order}", source.browse_path),
            LIST_FIXTURE,
            &base,
        );
        Ok(parse_listing(&body, source, &base, page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(&base) || query.contains("wfwf") {
            let key = normalize_key(query, source);
            return Ok(Paged {
                entries: vec![self.details(json!({"sourceId": source.id, "manga": key, "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?],
                has_next_page: false,
            });
        }
        if query.trim().is_empty() {
            return self.list(request);
        }
        if query.chars().filter(|ch| !ch.is_whitespace()).count() < 2 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        }
        let body = fetch_document(
            &format!("{base}/search.html?q={}", url::query_escape(query)),
            SEARCH_FIXTURE,
            &base,
        );
        Ok(parse_search(&body, source, &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_details(
            &fetch_document(&manga_url(&key, source, &base), DETAILS_FIXTURE, &base),
            &key,
            source,
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".to_string());
        Ok(parse_chapters(
            &fetch_document(&manga_url(&key, source, &base), DETAILS_FIXTURE, &base),
            source,
            &base,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| json!({"toon":"1","num":"1"}).to_string());
        Ok(parse_pages(
            &fetch_document(&chapter_url(&key, source, &base), PAGES_FIXTURE, &base),
            &base,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key, source, &base)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key, source, &base)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        let base = base_url(&request);
        if input.starts_with("http") && input.contains("wfwf") {
            let key = normalize_key(input, source);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE, &base),
                    &key,
                    source,
                    &base,
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

#[derive(Clone, Copy)]
struct WolfSource {
    id: &'static str,
    browse_path: &'static str,
    entry_path: &'static str,
    reader_path: &'static str,
}

const WEBTOON: WolfSource = WolfSource {
    id: "wolfdotcom-webtoon",
    browse_path: "ing",
    entry_path: "list",
    reader_path: "view",
};
const COMICBOOK: WolfSource = WolfSource {
    id: "wolfdotcom-comicbook",
    browse_path: "cm",
    entry_path: "cl",
    reader_path: "cv",
};
const PHOTOTOON: WolfSource = WolfSource {
    id: "wolfdotcom-phototoon",
    browse_path: "pt",
    entry_path: "list",
    reader_path: "view",
};

fn source_for(request: &Value) -> WolfSource {
    match request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
    {
        Some("wolfdotcom-comicbook") => COMICBOOK,
        Some("wolfdotcom-phototoon") => PHOTOTOON,
        _ => WEBTOON,
    }
}

fn base_url(request: &Value) -> String {
    let domain = request
        .get("preferences")
        .and_then(|prefs| {
            prefs
                .get("domainNumber")
                .or_else(|| prefs.get("domain_number"))
        })
        .and_then(Value::as_str)
        .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(DEFAULT_DOMAIN_NUMBER);
    format!("https://wfwf{domain}.com")
}

fn client(base: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str, base: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_listing(body: &str, source: WolfSource, base: &str, page: u64) -> Paged<CatalogItem> {
    let items = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("webtoon-list") || chunk.contains("toon=") || chunk.contains("subject")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let toon = query_param(&href, "toon")?;
            Some(CatalogItem {
                key: toon.clone(),
                title: html::text_between(chunk, "subject", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| format!("Wolf {toon}")),
                cover: html::attr_after(chunk, "<img", "data-original")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(base, &value)),
                url: Some(manga_url(&toon, source, base)),
                language: Some("ko".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    let start = page.saturating_sub(1) as usize * 20;
    let end = (start + 20).min(items.len());
    Paged {
        entries: if start < items.len() {
            items[start..end].to_vec()
        } else {
            Vec::new()
        },
        has_next_page: end < items.len(),
    }
}

fn parse_search(body: &str, source: WolfSource, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("searchItem")
        .skip(1)
        .filter(|chunk| chunk.contains(source.entry_path))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "searchLink", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let toon = query_param(&href, "toon")?;
            Some(CatalogItem {
                key: toon.clone(),
                title: html::text_between(chunk, "searchDetailTitle", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| format!("Wolf {toon}")),
                cover: style_image(chunk).map(|value| url::join_url(base, &value)),
                url: Some(manga_url(&toon, source, base)),
                language: Some("ko".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: &str, source: WolfSource, base: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Wolf {key}")),
        cover: html::attr_after(body, "img-box", "src").map(|value| url::join_url(base, &value)),
        description: html::text_between(body, "text-box", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: sub_value(body, "장르")
            .map(|value| split_slash(&value))
            .unwrap_or_default(),
        authors: sub_value(body, "작가")
            .map(|value| split_slash(&value))
            .unwrap_or_default(),
        url: Some(manga_url(key, source, base)),
        language: Some("ko".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: WolfSource, base: &str) -> Vec<MangaChapter> {
    body.split("view_open")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let toon = query_param(&href, "toon")?;
            let num = query_param(&href, "num")?;
            let key = json!({"toon": toon, "num": num}).to_string();
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "subject", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(parse_date),
                url: Some(chapter_url(&key, source, base)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("data-original") || chunk.contains("image-view"))
        .filter_map(|chunk| html::attr(chunk, "data-original").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(base, &image),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn manga_url(key: &str, source: WolfSource, base: &str) -> String {
    format!(
        "{base}/{}?toon={}",
        source.entry_path,
        query_escape_key(key)
    )
}

fn chapter_url(key: &str, source: WolfSource, base: &str) -> String {
    let value = serde_json::from_str::<Value>(key).unwrap_or(Value::Null);
    let toon = value.get("toon").and_then(Value::as_str).unwrap_or("1");
    let num = value.get("num").and_then(Value::as_str).unwrap_or("1");
    format!("{base}/{}?toon={toon}&num={num}", source.reader_path)
}

fn normalize_key(value: &str, _source: WolfSource) -> String {
    query_param(value, "toon").unwrap_or_else(|| value.trim_matches('/').to_string())
}

fn query_escape_key(key: &str) -> String {
    key.trim_matches('/').to_string()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    input.split('?').nth(1)?.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn style_image(input: &str) -> Option<String> {
    Some(
        input
            .split("background-image:url(")
            .nth(1)?
            .split(')')
            .next()?
            .trim_matches(['"', '\''])
            .to_string(),
    )
}

fn sub_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn split_slash(input: &str) -> Vec<String> {
    input
        .split('/')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_date(input: String) -> Option<i64> {
    let parts = input
        .split('-')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect::<Vec<_>>();
    if parts.len() == 3 {
        Some(parts[0] * 10_000 + parts[1] * 100 + parts[2])
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
<div class="webtoon-list"><ul><li><a href="/list?toon=1"><div class="img"><img data-original="/cover.jpg"></div><div class="txt"><span class="subject">Sample Wolf</span></div></a></li></ul></div>
"#;
const SEARCH_FIXTURE: &str = r#"
<article class="searchItem"><a class="searchLink" href="/list?toon=1"><div class="searchPng" style="background-image:url(/cover.jpg)"></div><div class="searchDetailTitle">Sample Wolf</div></a></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="img-box"><img src="/cover.jpg"></div><div class="text-box"><h1>Sample Wolf</h1><div class="txt">Description</div><div class="sub"><strong>장르</strong>액션/판타지</div><div class="sub"><strong>작가</strong>Author</div></div>
<div class="webtoon-bbs-list"><a class="view_open" href="/view?toon=1&num=1"><span class="subject">Chapter 1</span><span class="date">2024-01-01</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="image-view"><img data-original="/page1.jpg"></div>"#;
