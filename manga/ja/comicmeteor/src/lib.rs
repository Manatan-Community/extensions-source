use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, ProcessedImage, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    dates, html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url,
};
use serde_json::{Value, json};

const SOURCE: ComicMeteor = ComicMeteor;
const BASE_URL: &str = "https://kirapo.jp";
const API_URL: &str = "https://kirapo.jp/api";

struct ComicMeteor;

impl MangaSource for ComicMeteor {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.search(json!({"page": page(&request), "query": ""}))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).filter(|key| is_manga_key(key)) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }

        let target = if !query.is_empty() {
            format!("{BASE_URL}/search?word={}", url::query_escape(query))
        } else if let Some(filter) = filter_string(&request, "browse").filter(|value| !value.is_empty()) {
            format!("{BASE_URL}/titles?{filter}")
        } else {
            format!("{BASE_URL}/titles")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), &target))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        let reader_url = absolute_url(&key);
        let body = fetch_document(&reader_url, READER_FIXTURE);
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: false,
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::SpeedBinb::process_page_image(request)
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
        if let Some(key) = key_from_url(input).filter(|key| is_manga_key(key)) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn fetch_api(target: &str) -> Option<Value> {
    client()
        .get(target)
        .send_text()
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
}

fn parse_listing(body: &str, target: &str) -> Paged<CatalogItem> {
    let mut entries = if target.contains("/search") {
        parse_search_items(body)
    } else {
        parse_title_items(body)
    };
    if !target.contains("/search") {
        entries.extend(api_more_items(body, target));
    }
    entries = entries.into_iter().fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_search_items(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("w-auto") || chunk.contains("grid-group"))
        .filter_map(link_item)
        .collect()
}

fn parse_title_items(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/title/"))
        .filter_map(link_item)
        .collect()
}

fn link_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    let image = html::attr_after(chunk, "<img", "src");
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<img", "alt")
            .or_else(|| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Kiraboshi".into())),
        cover: image.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn api_more_items(body: &str, target: &str) -> Vec<CatalogItem> {
    let Some(read_at) = html::attr_after(body, "more_titles_button", "data-read-at") else {
        return Vec::new();
    };
    let mut api = format!("{API_URL}/title-list?read_at={}", url::query_escape(&read_at));
    if let Some((_, query)) = target.split_once('?') {
        for pair in query.split('&').filter(|pair| !pair.starts_with("read_at=")) {
            if !pair.is_empty() {
                api.push('&');
                api.push_str(pair);
            }
        }
    }
    let Some(root) = fetch_api(&api) else {
        return Vec::new();
    };
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url_value = item.get("url").and_then(Value::as_str)?;
            let key = normalize_key(url_value);
            Some(CatalogItem {
                key: key.clone(),
                title: item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Kiraboshi".into())),
                cover: item
                    .get("thumbnail")
                    .and_then(Value::as_str)
                    .map(|value| absolute_url(value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), &key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "<main", "</main>")
            .and_then(|main| html::text_between(&main, "<h2", "</h2>"))
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Kiraboshi".into())),
        cover: html::attr_after(body, "<main", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        authors: anchor_texts_with_href(body, "/authors/"),
        description: html::text_between(body, "id=\"plot\"", "</div>")
            .and_then(|_| {
                body.split("id=\"plot\"")
                    .nth(1)
                    .and_then(|chunk| html::text_between(chunk, "<div", "</div>"))
            })
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: anchor_texts_with_class(body, "button-gray"),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("episode-item")
        .skip(1)
        .filter(|chunk| !chunk.contains("未公開話"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "episode-item-left", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        if let Some(href) = html::attr_after(body, "episode-read", "href") {
            chapters.push(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(body, "latest-episode-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: parse_date(body),
                ..MangaChapter::default()
            });
        }
    }
    chapters
}

fn parse_date(body: &str) -> Option<i64> {
    html::text_between(body, "last-update", "</")
        .map(|value| html::strip_tags(&value))
        .and_then(|value| {
            value
                .split("更新")
                .next()
                .map(str::trim)
                .map(|date| date.replace('年', "-").replace('月', "-").replace('日', ""))
        })
        .and_then(|value| dates::parse_ymd(&value))
}

fn anchor_texts_with_href(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| html::attr(chunk, "href").is_some_and(|href| href.contains(href_part)))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn anchor_texts_with_class(body: &str, class_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| html::attr(chunk, "class").is_some_and(|class| class.contains(class_part)))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn is_manga_key(key: &str) -> bool {
    key.contains("/title/")
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut entries: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="titles-container">
  <a href="/title/sample"><img src="/cover.jpg" alt="Sample Kiraboshi"></a>
</div>
<button id="more_titles_button" data-read-at=""></button>
"#;

const DETAILS_FIXTURE: &str = r#"
<main><h2>Sample Kiraboshi</h2><img src="/cover.jpg"></main>
<a href="/authors/sample">Sample Author</a>
<div id="plot"></div><div>Sample description.</div>
<div class="pt-5"><a class="button-gray">Sample Tag</a></div>
<div class="episodes-container">
  <div class="episode-item"><a href="/episode/sample"><span class="episode-item-left">Episode 1</span></a></div>
</div>
"#;

const READER_FIXTURE: &str = r#"
<div id="content"><img data-ptimg="/sample.ptimg.json"></div>
"#;
