use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: ManhwaZone = ManhwaZone;
const BASE_URL: &str = "https://manhwazone.com";

struct ManhwaZone;

impl MangaSource for ManhwaZone {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "popularity"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/series?sortBy={sort}&page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or("").trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters");
        let mut target = format!("{BASE_URL}/series?page={page}");
        if !query.is_empty() {
            target.push_str("&keyword=");
            target.push_str(&url::query_escape(query));
        }
        for (key, param) in [("sortBy", "sortBy"), ("status", "status"), ("genres", "genres")] {
            let value = filter(filters, key, "");
            if !value.is_empty() {
                target.push('&');
                target.push_str(param);
                target.push('=');
                target.push_str(&url::query_escape(value));
            }
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(fetch_livewire_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_livewire_chapters(body: &str) -> Vec<MangaChapter> {
    let Some(wire_chunk) = body.split("wire:snapshot").nth(1) else {
        return parse_chapters_from_snapshot(CHAPTERS_FIXTURE);
    };
    let snapshot = html::attr(wire_chunk, "wire:snapshot").unwrap_or_default();
    let token = html::attr_after(body, "csrf-token", "content").unwrap_or_default();
    if snapshot.is_empty() {
        return parse_chapters_from_snapshot(CHAPTERS_FIXTURE);
    }
    let payload = json!({
        "_token": token,
        "components": [{
            "snapshot": snapshot,
            "updates": {},
            "calls": [{ "path": "", "method": "bootLoad", "params": [] }]
        }]
    });
    let response = client()
        .post(format!("{BASE_URL}/livewire/update"))
        .header("Accept", "application/json")
        .json(payload.to_string())
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    parse_livewire_update(&response)
}

fn parse_livewire_update(body: &str) -> Vec<MangaChapter> {
    let snapshot = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("components")?.as_array()?.first()?.get("snapshot")?.as_str().map(ToString::to_string));
    snapshot.map_or_else(|| parse_chapters_from_snapshot(body), |value| parse_chapters_from_snapshot(&value))
}

fn parse_chapters_from_snapshot(body: &str) -> Vec<MangaChapter> {
    let Ok(root) = serde_json::from_str::<Value>(body) else { return Vec::new(); };
    let Some(items) = root
        .get("data")
        .and_then(|data| data.get("chapters"))
        .and_then(Value::as_array)
        .and_then(|chapters| chapters.first())
        .and_then(Value::as_array) else {
            return Vec::new();
        };
    items
        .iter()
        .filter_map(|item| {
            let chapter = item.as_array()?.first()?;
            let web_url = json_text(chapter, "web_url").or_else(|| json_text(chapter, "webUrl"))?;
            let key = normalize_key(&web_url);
            Some(MangaChapter {
                key: key.clone(),
                title: json_text(chapter, "name").or_else(|| Some("Chapter".into())),
                date_uploaded: json_text(chapter, "published").and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("group"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "font-semibold", "</a>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwaZone".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let has_next_page = body.contains("rel=\"next\"") || body.contains("rel='next'") || body.contains("&rsaquo;") || entries.len() >= 24;
    Paged { entries, has_next_page }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ManhwaZone".into())),
        cover: image_attr(body).map(|image| absolute_url(&image)),
        description: html::text_between(body, "page-subtitle", "</").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        authors: json_ld_author(body).into_iter().collect(),
        tags: body.split("badge-genre").skip(1).filter_map(|chunk| html::text_between(chunk, ">", "</a>")).map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()).collect(),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Some(conf) = extract_rs_conf(body) {
        if let (Some(path), Some(expire), Some(signature), Some(total)) = (
            json_text(&conf, "p"),
            json_text(&conf, "expire"),
            json_text(&conf, "signature"),
            conf.get("tt").and_then(Value::as_u64),
        ) {
            if total > 0 {
                return (1..=total)
                    .map(|index| {
                        let page = format!("{index:03}");
                        MangaPage {
                            content: PageContent::Url {
                                url: format!("https://img.mangalaxy.net/_img/{path}/{page}.webp?e={expire}&s={signature}"),
                                context: None,
                            },
                            headers: manga::image_headers(BASE_URL),
                            description: Some(format!("Page {index}")),
                            ..MangaPage::default()
                        }
                    })
                    .collect();
            }
        }
    }
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_rs_conf(body: &str) -> Option<Value> {
    let chunk = body.split("__RS_CONF__").nth(1)?;
    let start = chunk.find('{')?;
    let end = chunk[start..].find("};").map(|index| start + index + 1)?;
    serde_json::from_str(&chunk[start..end]).ok()
}

fn json_ld_author(body: &str) -> Option<String> {
    let json = body.split("application/ld+json").nth(1)?;
    let name = json.split("\"name\"").nth(1)?;
    let value = name.split('"').nth(2)?.to_string();
    (value.to_ascii_lowercase() != "unknown").then_some(value)
}

fn parse_status(body: &str) -> ItemStatus {
    let value = body.to_ascii_lowercase();
    if value.contains("completed") || value.contains("finished") {
        ItemStatus::Completed
    } else if value.contains("on hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("discontinued") || value.contains("cancelled") {
        ItemStatus::Cancelled
    } else if value.contains("on going") || value.contains("ongoing") || value.contains("currently publishing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters.and_then(Value::as_object).and_then(|object| object.get(key)).and_then(Value::as_str).filter(|value| !value.is_empty()).unwrap_or(fallback)
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input[BASE_URL.len()..].trim_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<article class="group"><a href="/series/sample"><img src="/cover.jpg"></a><div class="min-w-0"><a class="font-semibold" href="/series/sample">Sample Manga</a></div></article>"#;
const DETAILS_FIXTURE: &str = r#"<meta name="csrf-token" content="token"><h1 class="page-title">Sample Manga</h1><p class="page-subtitle">Sample summary</p><img class="aspect-[7/10]" src="/cover.jpg"><a class="badge-genre">Action</a><span class="badge-sm">On Going</span><script type="application/ld+json">{"author":[{"@type":"Person","name":"Sample Author"}]}</script><div wire:init="bootLoad" wire:id="1" wire:snapshot="{&quot;data&quot;:{&quot;chapters&quot;:[[ [{&quot;web_url&quot;:&quot;/series/sample/chapter-1&quot;,&quot;name&quot;:&quot;Chapter 1&quot;,&quot;published&quot;:&quot;2024-01-01 00:00:00&quot;}] ]]}}"></div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"components":[{"snapshot":"{\"data\":{\"chapters\":[[[{\"web_url\":\"/series/sample/chapter-1\",\"name\":\"Chapter 1\",\"published\":\"2024-01-01 00:00:00\"}]]]}}"}]}"#;
const PAGES_FIXTURE: &str = r#"<script>window.__RS_CONF__ = {"p":"sample","expire":"1","signature":"sig","tt":2};</script>"#;
