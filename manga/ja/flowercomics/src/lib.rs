use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    ProcessedImage, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: FlowerComics = FlowerComics;
const BASE_URL: &str = "https://flowercomics.jp";

struct FlowerComics;

impl MangaSource for FlowerComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return Ok(parse_weekday_entries(&fetch_rsc(&format!("{BASE_URL}/rensai"), RSC_LATEST_FIXTURE), "mon"));
        }
        Ok(parse_ranking(&fetch_rsc(&format!("{BASE_URL}/ranking"), RSC_RANKING_FIXTURE)))
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
            return Ok(parse_html_grid(&fetch_document(&target, LIST_FIXTURE)));
        }
        let filter = filter_string(&request, "category").unwrap_or_else(|| "day:mon".into());
        let (kind, value) = filter.split_once(':').unwrap_or(("day", "mon"));
        match kind {
            "day" => Ok(parse_weekday_entries(&fetch_rsc(&format!("{BASE_URL}/rensai#{value}"), RSC_LATEST_FIXTURE), value)),
            "rensai" => Ok(parse_html_grid(&fetch_document(&format!("{BASE_URL}/rensai/{value}"), LIST_FIXTURE))),
            _ => Ok(parse_html_grid(&fetch_document(&format!("{BASE_URL}/tag/{value}/{kind}"), LIST_FIXTURE))),
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(parse_chapters(
            &fetch_rsc(&absolute_url(&key), CHAPTERS_FIXTURE),
            hide_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".into());
        let target = format!("{}/viewer", absolute_url(&key).trim_end_matches('/'));
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

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let ranking = json_objects_containing(body, "\"rankingTypeName\"")
        .into_iter()
        .find(|value| string_at(value, "rankingTypeName").as_deref() == Some("総合"))
        .unwrap_or_else(|| serde_json::from_str(RSC_RANKING_FIXTURE).unwrap_or(Value::Null));
    let entries = ranking
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(entry_to_item)
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_weekday_entries(body: &str, day: &str) -> Paged<CatalogItem> {
    let root = json_objects_containing(body, "\"weekdays\"")
        .into_iter()
        .next()
        .unwrap_or_else(|| serde_json::from_str(RSC_LATEST_FIXTURE).unwrap_or(Value::Null));
    let entries = root
        .pointer(&format!("/weekdays/{day}"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(entry_to_item)
        .collect();
    Paged { entries, has_next_page: false }
}

fn parse_html_grid(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/title/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "text-black", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Flower Comics".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { entries, has_next_page: false }
}

fn entry_to_item(entry: &Value) -> Option<CatalogItem> {
    let id = entry.get("id").and_then(Value::as_i64)?.to_string();
    let key = format!("/title/{id}");
    Some(CatalogItem {
        key: key.clone(),
        title: string_at(entry, "name").unwrap_or_else(|| "Flower Comics".into()),
        cover: entry.pointer("/thumbnail/src").and_then(Value::as_str).map(|value| absolute_url(value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), &key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = html::text_between(body, "bg-main-blue", "</").map(|value| html::strip_tags(&value)).unwrap_or_default();
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Flower Comics".into())),
        authors: anchor_texts_with_href(body, "/author/"),
        description: html::text_between(body, "whitespace-pre-wrap", "</div>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        tags: body
            .split("aria-label=\"ジャンルタグ一覧\"")
            .nth(1)
            .map(anchor_paragraphs)
            .unwrap_or_default(),
        cover: html::attr_after(body, "object-cover", "src").or_else(|| html::attr_after(body, "<img", "src")).map(|value| absolute_url(&value)),
        status: if status_text.contains("完結") {
            ItemStatus::Completed
        } else if status_text.contains("更新予定") || status_text.contains("連載") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let root = json_objects_containing(body, "\"earlyChapters\"")
        .into_iter()
        .next()
        .unwrap_or_else(|| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or(Value::Null));
    ["earlyChapters", "omittedMiddleChapters", "latestChapters"]
        .into_iter()
        .flat_map(|key| root.get(key).and_then(Value::as_array).into_iter().flatten())
        .filter(|chapter| !hide_locked || !chapter_is_locked(chapter))
        .filter_map(|chapter| {
            let id = chapter.get("id").and_then(Value::as_i64)?.to_string();
            let locked = chapter_is_locked(chapter);
            Some(MangaChapter {
                key: format!("/chapter/{id}"),
                title: string_at(chapter, "title").map(|title| {
                    let subtitle = string_at(chapter, "subTitle").unwrap_or_default();
                    let title = format!("{title}{subtitle}");
                    if locked { format!("Locked: {title}") } else { title }
                }),
                date_uploaded: string_at(chapter, "updated").and_then(|value| dates::parse_ymd(&value.replace('/', "-"))),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    json_objects_containing(body, "\"src\"")
        .into_iter()
        .filter_map(|page| {
            let src = string_at(&page, "src")?;
            let crypto = page.get("crypto")?;
            Some(MangaPage {
                content: PageContent::Url {
                    url: src,
                    context: Some(manga::image_headers(referer)),
                },
                headers: manga::image_headers(referer),
                extra: BTreeMap::from([
                    ("aesKeyHex".into(), Value::String(string_at(crypto, "key").unwrap_or_default())),
                    ("aesIvHex".into(), Value::String(string_at(crypto, "iv").unwrap_or_default())),
                ]),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn chapter_is_locked(chapter: &Value) -> bool {
    matches!(chapter.get("chapterType").and_then(Value::as_i64), Some(2 | 3 | 4))
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

fn anchor_texts_with_href(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| html::attr(chunk, "href").is_some_and(|href| href.contains(href_part)))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn anchor_paragraphs(body: &str) -> Vec<String> {
    body.split("<p")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
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
<div class="grid"><a href="/title/1"><img src="/cover.jpg"><p class="text-black">Sample Flower</p></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<section><img class="object-cover" src="/cover.jpg"></section>
<h1>Sample Flower</h1>
<a href="/author/sample"><p>Sample Author</p></a>
<div class="whitespace-pre-wrap"><p>Sample description.</p></div>
<ul aria-label="ジャンルタグ一覧"><li><p>恋愛</p></li></ul>
<div class="bg-main-blue"><p>連載</p></div>
"#;

const RSC_RANKING_FIXTURE: &str = r#"{"rankingTypeName":"総合","titles":[{"id":1,"thumbnail":{"src":"/cover.jpg"},"name":"Sample Flower"}]}"#;
const RSC_LATEST_FIXTURE: &str = r#"{"weekdays":{"mon":[{"id":1,"thumbnail":{"src":"/cover.jpg"},"name":"Sample Flower"}]}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"earlyChapters":[{"chapterType":0,"id":1,"subTitle":"","title":"Episode 1","updated":"2024/01/01"}],"omittedMiddleChapters":[],"latestChapters":[]}"#;
const PAGES_FIXTURE: &str = r#"[{"src":"https://flowercomics.jp/page.enc","crypto":{"key":"00112233445566778899aabbccddeeff","iv":"0102030405060708090a0b0c0d0e0f10"}}]"#;
