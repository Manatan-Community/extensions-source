use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: ThunderScans = ThunderScans;

struct ThunderScans;

impl MangaSource for ThunderScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = search_url(
            source,
            "",
            page_for(&request),
            if latest { "update" } else { "popular" },
            &Value::Null,
        );
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            source,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(source.base_url) {
            return Ok(Paged {
                entries: vec![catalog_from_url(query, source, None, None)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let order = filters
            .get("order")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = search_url(source, query, page_for(&request), order, filters);
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            source,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("{}/sample/", source.manga_path));
        let body =
            fetch_document_or_fixture(&url::join_url(source.base_url, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, &key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("{}/sample/", source.manga_path));
        let hide_paid = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hidePaidChapters"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let body =
            fetch_document_or_fixture(&url::join_url(source.base_url, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, source, hide_paid))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapter/".to_string());
        let body = fetch_document_or_fixture(&url::join_url(source.base_url, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body, source))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let source = source_for(&request);
        let popular = self
            .list(serde_json::json!({"sourceId": source.id, "listingId": "popular", "page": 1}))?;
        let latest = self
            .list(serde_json::json!({"sourceId": source.id, "listingId": "latest", "page": 1}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(source.base_url, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let source = source_for(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(source.base_url, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(source.base_url) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_url(input, source, None, None)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    lang: &'static str,
    manga_path: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "lavascans-ar",
        name: "Lava Scans",
        base_url: "https://lavascans.com",
        lang: "ar",
        manga_path: "/manga",
    },
    SourceConfig {
        id: "thunderscans-en",
        name: "Thunder Scans",
        base_url: "https://en-thunderscans.com",
        lang: "en",
        manga_path: "/comics",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("thunderscans-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[1])
}

fn client(source: SourceConfig) -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{}/", source.base_url))
        .with_cookies_for(source.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    let source = SOURCES
        .iter()
        .copied()
        .find(|source| target.starts_with(source.base_url))
        .unwrap_or(SOURCES[1]);
    client(source)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(
    source: SourceConfig,
    query: &str,
    page: u64,
    order: &str,
    filters: &Value,
) -> String {
    let mut target = format!(
        "{}/{}?title={}&page={}",
        source.base_url.trim_end_matches('/'),
        source.manga_path.trim_matches('/'),
        url::query_escape(query),
        page
    );
    if !order.is_empty() {
        target.push_str("&order=");
        target.push_str(&url::query_escape(order));
    }
    for (id, param) in [("author", "author"), ("year", "yearx")] {
        if let Some(value) = filters
            .get(id)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            target.push('&');
            target.push_str(param);
            target.push('=');
            target.push_str(&url::query_escape(value));
        }
    }
    target.push('/');
    target
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("bsx") || chunk.contains("manga-card-v") || chunk.contains("imgu")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "bigor", "</")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Manga".to_string())
                });
            let cover = html::attr_after(chunk, "<img", "data-src")
                .or_else(|| html::attr_after(chunk, "<img", "src"))
                .map(|value| url::join_url(source.base_url, &value));
            Some(catalog_from_url(&href, source, Some(title), cover))
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("pagination")
            && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_details(body: &str, key: &str, source: SourceConfig) -> CatalogItem {
    let mut item = catalog_from_url(&url::join_url(source.base_url, key), source, None, None);
    item.title = html::text_between(body, "entry-title", "</")
        .or_else(|| html::text_between(body, "lh-title", "</"))
        .or_else(|| html::text_between(body, "ts-breadcrumb", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    item.cover = html::attr_after(body, "lh-poster", "src")
        .or_else(|| html::attr_after(body, "thumb", "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|value| url::join_url(source.base_url, &value));
    item.description = html::text_between(body, "manga-story", "</div>")
        .or_else(|| html::text_between(body, "entry-content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.tags = collect_links(body, "gnr")
        .into_iter()
        .chain(collect_links(body, "lh-genres"))
        .collect();
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, source: SourceConfig, hide_paid: bool) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .chain(body.split("<div").skip(1))
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter")
                || chunk.contains("ch-item")
                || chunk.contains("eph-num")
                || chunk.contains("chapternum")
        })
        .filter(|chunk| !hide_paid || !(chunk.contains("locked") || chunk.contains("paid")))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "ch-num", "</")
                .or_else(|| html::text_between(chunk, "chapternum", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href, source),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "ch-date", "</")
                    .or_else(|| html::text_between(chunk, "chapterdate", "</"))
                    .and_then(|date| parse_date(&html::strip_tags(&date))),
                url: Some(url::join_url(
                    source.base_url,
                    &normalize_key(&href, source),
                )),
                is_locked: chunk.contains("locked") || chunk.contains("paid"),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, source: SourceConfig) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "src"))
                .filter(|value| !value.is_empty() && !value.contains("logo"))
        })
        .collect::<Vec<_>>();
    if pages.is_empty() {
        if let Some(json) = body
            .split("ts_reader.run")
            .nth(1)
            .and_then(|tail| tail.split('[').nth(1))
            .and_then(|tail| tail.split(']').next())
        {
            pages = json
                .split('"')
                .filter(|part| part.starts_with("http"))
                .map(ToString::to_string)
                .collect();
        }
    }
    pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(source.base_url, &image),
                context: Some(manga::image_headers(source.base_url)),
            },
            description: Some((index + 1).to_string()),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_from_url(
    input: &str,
    source: SourceConfig,
    title: Option<String>,
    cover: Option<String>,
) -> CatalogItem {
    let key = normalize_key(input, source);
    CatalogItem {
        key: key.clone(),
        title: title.unwrap_or_else(|| {
            url::slug_from_url(&key)
                .unwrap_or_else(|| source.name.to_string())
                .replace('-', " ")
        }),
        cover,
        url: Some(url::join_url(source.base_url, &key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str, source: SourceConfig) -> String {
    if input.starts_with(source.base_url) {
        input
            .trim_start_matches(source.base_url)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
}

fn collect_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .flat_map(|chunk| chunk.split("<a").skip(1).take(8))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_date(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some(date) = value
        .split_whitespace()
        .next()
        .filter(|part| part.contains('/'))
    {
        let mut parts = date.split('/').filter_map(|part| part.parse::<i64>().ok());
        return Some(days_from_civil(parts.next()?, parts.next()?, parts.next()?) * 86_400);
    }
    None
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn page_for(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bs"><div class="bsx"><a href="https://en-thunderscans.com/comics/sample/" title="Sample Thunder"><img src="/cover.jpg"></a><div class="bigor"><div class="tt">Sample Thunder</div></div></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample Thunder</h1><div class="thumb"><img src="/cover.jpg"></div><div class="entry-content">Sample description</div><div class="gnr"><a>Action</a></div></div>
<div id="chapterlist"><li class="wp-manga-chapter"><a href="https://en-thunderscans.com/sample-chapter/">Chapter 1</a><span class="chapterdate">2024/01/01</span></li></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="readerarea"><img src="https://cdn.example/page-1.jpg"><img data-src="https://cdn.example/page-2.jpg"></div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE, SOURCES[1]);
        assert_eq!(page.entries[0].key, "/comics/sample/");
        assert_eq!(page.entries[0].title, "Sample Thunder");
    }

    #[test]
    fn parses_details_chapters_and_pages() {
        let item = parse_details(DETAILS_FIXTURE, "/comics/sample/", SOURCES[1]);
        assert_eq!(item.title, "Sample Thunder");
        let chapters = parse_chapters(DETAILS_FIXTURE, SOURCES[1], true);
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        let pages = parse_pages(PAGES_FIXTURE, SOURCES[1]);
        assert_eq!(pages.len(), 2);
    }
}
