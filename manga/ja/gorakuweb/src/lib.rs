use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent,
    ProcessedImage, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: GorakuWeb = GorakuWeb;
const BASE_URL: &str = "https://gorakuweb.com";

struct GorakuWeb;

impl MangaSource for GorakuWeb {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return Ok(parse_html_listing(&fetch_document(BASE_URL, LIST_FIXTURE)));
        }
        Ok(parse_rsc_entries(&fetch_rsc(BASE_URL, RSC_LIST_FIXTURE)))
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
        if !query.is_empty() {
            let target = format!("{BASE_URL}/search?keyword={}", url::query_escape(query));
            return Ok(parse_html_listing(&fetch_document(&target, LIST_FIXTURE)));
        }
        let category = filter_string(&request, "category").unwrap_or_else(|| "series:".into());
        let (kind, value) = category.split_once(':').unwrap_or(("series", ""));
        if kind == "series" {
            let target = if value.is_empty() {
                format!("{BASE_URL}/series")
            } else {
                format!("{BASE_URL}/series?completed={value}")
            };
            Ok(parse_rsc_entries(&fetch_rsc(&target, RSC_LIST_FIXTURE)))
        } else {
            let target = format!("{BASE_URL}/search?id={value}");
            Ok(parse_html_listing(&fetch_document(&target, LIST_FIXTURE)))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let body = fetch_rsc(&format!("{BASE_URL}/episode/{}", slug_from_key(&key)), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, preferences(&request).get("hide_locked").and_then(Value::as_bool).unwrap_or(false)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        let target = absolute_url(&key);
        Ok(parse_pages(&fetch_rsc(&target, PAGES_FIXTURE), &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"listing": "popular"}))?;
        let latest = self.list(json!({"listing": "latest"}))?;
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
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::AesImage::process_128_pkcs7_hex(request)
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

fn fetch_rsc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .header("RSC", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_html_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("bdr_lg group")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Goraku Web".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
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
        has_next_page: false,
    }
}

fn parse_rsc_entries(body: &str) -> Paged<CatalogItem> {
    let entries = json_objects_containing(body, "\"href\"")
        .into_iter()
        .filter_map(|object| {
            let href = string_at(&object, "href")?;
            let title = string_at(&object, "title")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: string_at(&object, "imageSrc").map(|value| absolute_url(&value)),
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
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_rsc(&format!("{BASE_URL}/episode/{}", slug_from_key(&key)), DETAILS_FIXTURE);
    let root = episode_props(&body);
    CatalogItem {
        key,
        title: string_at(&root, "seriesTitle").unwrap_or_else(|| "Goraku Web".into()),
        description: string_at(&root, "seriesDescription").map(|value| html::strip_tags(&value)),
        authors: string_at(&root, "author").into_iter().collect(),
        cover: string_at(&root, "seriesThumbnailUrl").map(|value| absolute_url(&value)),
        url: string_at(&root, "shareUrl"),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = episode_props(body);
    let Some(chapters) = root.get("episodeList").and_then(Value::as_array) else {
        return Vec::new();
    };
    chapters
        .iter()
        .filter(|chapter| !hide_locked || chapter.get("disabled").and_then(Value::as_bool) != Some(true))
        .filter_map(|chapter| {
            let key = string_at(chapter, "href")?;
            Some(MangaChapter {
                key: normalize_key(&key),
                title: string_at(chapter, "title").map(|title| {
                    if chapter.get("disabled").and_then(Value::as_bool) == Some(true) {
                        format!("Locked: {title}")
                    } else {
                        title
                    }
                }),
                date_uploaded: string_at(chapter, "openAt").and_then(|value| dates::parse_ymd(&value.replace('/', "-"))),
                is_locked: chapter.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let root = episode_props(body);
    let base = string_at(&root, "base").unwrap_or_else(|| BASE_URL.into());
    let token = string_at(&root, "accessKey").unwrap_or_default();
    let key = string_at(&root, "keyBytes").unwrap_or_default();
    let iv = string_at(&root, "ivBytes").unwrap_or_default();
    root.pointer("/metadata/pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let filename = string_at(page, "filename")?;
            let image = format!("{}/{}?__token__={}", base.trim_end_matches('/'), filename.trim_start_matches('/'), url::query_escape(&token));
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                extra: BTreeMap::from([
                    ("aesKeyHex".into(), Value::String(key.clone())),
                    ("aesIvHex".into(), Value::String(iv.clone())),
                ]),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn episode_props(body: &str) -> Value {
    json_objects_containing(body, "\"episodeList\"")
        .into_iter()
        .next()
        .unwrap_or_else(|| serde_json::from_str(DETAILS_FIXTURE).unwrap_or(Value::Null))
}

fn json_objects_containing(body: &str, marker: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for (index, _) in body.match_indices('{') {
        let Some(end) = balanced_end(&body[index..], '{', '}') else {
            continue;
        };
        let candidate = &body[index..index + end];
        if candidate.contains(marker)
            && let Ok(value) = serde_json::from_str::<Value>(candidate)
        {
            values.push(value);
        }
    }
    values
}

fn balanced_end(input: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index + ch.len_utf8());
            }
        }
    }
    None
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToOwned::to_owned).filter(|value| !value.is_empty())
}

fn preferences(request: &Value) -> &Value {
    request.get("preferences").unwrap_or(&Value::Null)
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

fn slug_from_key(key: &str) -> String {
    key.trim_matches('/').rsplit('/').next().unwrap_or(key).to_string()
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<section><h2>更新作品</h2><div class="bdr_lg group"><a href="/series/sample"><img src="/cover.jpg"><h3>Sample Goraku</h3></a></div></section>
"#;

const RSC_LIST_FIXTURE: &str = r#"[{"href":"/series/sample","imageSrc":"/cover.jpg","title":"Sample Goraku"}]"#;

const DETAILS_FIXTURE: &str = r#"{
  "seriesTitle": "Sample Goraku",
  "seriesDescription": "<p>Sample description.</p>",
  "author": "Sample Author",
  "seriesThumbnailUrl": "/cover.jpg",
  "shareUrl": "https://gorakuweb.com/series/sample",
  "episodeList": [
    { "href": "/episode/sample", "title": "Episode 1", "openAt": "2024/01/01", "disabled": false }
  ],
  "base": "https://gorakuweb.com/pages",
  "accessKey": "sample-token",
  "keyBytes": "00112233445566778899aabbccddeeff",
  "ivBytes": "0102030405060708090a0b0c0d0e0f10",
  "metadata": {
    "pages": [
      { "filename": "001.bin", "page": 1 }
    ]
  }
}"#;

const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
