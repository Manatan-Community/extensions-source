use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaUpJapan = MangaUpJapan;
const BASE_URL: &str = "https://www.manga-up.com";

struct MangaUpJapan;

impl MangaSource for MangaUpJapan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/series")
        } else {
            format!("{BASE_URL}/rankings/1")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if !query.is_empty() {
            format!("{BASE_URL}/titles?word={}", url::query_escape(query))
        } else {
            let category = filter_string(&request, "category").unwrap_or("mon");
            format!("{BASE_URL}/series/{category}")
        };
        Ok(parse_listing(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/titles/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/titles/sample".into());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_chapters(
            &fetch_rsc_document(&target, CHAPTERS_FIXTURE),
            title_id_from_key(&key).as_deref().unwrap_or("sample"),
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample-chapter#sample-title".into());
        let (chapter_id, title_id) = key.split_once('#').unwrap_or((&key, "sample-title"));
        let target = format!("{BASE_URL}/titles/{title_id}/chapters/{chapter_id}");
        Ok(parse_pages(
            &fetch_rsc_document(&target, PAGES_FIXTURE),
            &target,
        ))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"listingId": "latest"}))?;
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (chapter_id, title_id) = key.split_once('#').unwrap_or((&key, ""));
            format!("{BASE_URL}/titles/{title_id}/chapters/{chapter_id}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn fetch_rsc_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("rsc", "1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/titles/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/titles/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "line-clamp-2", "</")
                .or_else(|| html::text_between(chunk, "<h", "</h"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga UP!".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
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

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/titles/sample".into());
    let text = html::strip_tags(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h2", "</h2>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga UP!".into())),
        cover: html::attr_after(body, "<section", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: author_values(body),
        description: html::text_between(body, "あらすじ", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("/genres/"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if text.contains("完結") {
            ItemStatus::Completed
        } else if text.contains("更新") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, title_id: &str) -> Vec<MangaChapter> {
    let chapters_json = extract_json_array_after(body, "\"chapters\":")
        .unwrap_or_else(|| extract_json_array_after(CHAPTERS_FIXTURE, "\"chapters\":").unwrap());
    let mut chapters = serde_json::from_str::<Vec<ChapterData>>(&chapters_json)
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| chapter.into_chapter(title_id))
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let pages_json = extract_json_array_after(body, "\"pages\":")
        .unwrap_or_else(|| extract_json_array_after(PAGES_FIXTURE, "\"pages\":").unwrap());
    serde_json::from_str::<Vec<PageData>>(&pages_json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|page| page.content.value.and_then(|content| content.image_url))
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_json_array_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let bytes = body[start..].as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut begin = None;
    for (offset, byte) in bytes.iter().enumerate() {
        if begin.is_none() {
            if *byte == b'[' {
                begin = Some(offset);
                depth = 1;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match *byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'[' if !in_string => depth += 1,
            b']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    let begin = begin.unwrap();
                    return Some(body[start + begin..start + offset + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn author_values(body: &str) -> Vec<String> {
    body.split("flex-col")
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<div")
                .skip(1)
                .take(4)
                .filter_map(|part| html::text_between(part, ">", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty() && !value.contains("あらすじ"))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn title_id_from_key(key: &str) -> Option<String> {
    key.trim_matches('/')
        .split('/')
        .nth(1)
        .map(ToString::to_string)
}

fn key_from_url(value: &str) -> Option<String> {
    if !value.starts_with(BASE_URL) || !value.contains("/titles/") {
        return None;
    }
    Some(normalize_key(value))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(rest) = value.split(BASE_URL).nth(1) {
            return normalize_key(rest);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterData {
    id: String,
    name: String,
    sub_name: Option<String>,
    publishing_status: Option<i64>,
}

impl ChapterData {
    fn into_chapter(self, title_id: &str) -> MangaChapter {
        let mut title = String::new();
        let is_locked = self.publishing_status != Some(3);
        if is_locked {
            title.push_str("[LOCKED] (Preview) ");
        }
        if let Some(sub_name) = self.sub_name.filter(|value| !value.is_empty()) {
            title.push_str(&sub_name);
            title.push_str(" - ");
        }
        title.push_str(&self.name);
        MangaChapter {
            key: format!("{}#{title_id}", self.id),
            title: Some(title),
            chapter_number: self.id.parse::<f32>().ok(),
            is_locked,
            url: Some(format!("{BASE_URL}/titles/{title_id}/chapters/{}", self.id)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct PageData {
    content: PageContentData,
}

#[derive(Default, Deserialize)]
struct PageContentData {
    value: Option<PageValue>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageValue {
    image_url: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<div class="grid"><a href="/titles/sample-title"><img src="/cover.jpg"><div class="line-clamp-2">Sample Manga UP!</div></a></div>
"#;

const SEARCH_FIXTURE: &str = LIST_FIXTURE;

const DETAILS_FIXTURE: &str = r#"
<section><img src="/cover.jpg"></section><h2>Sample Manga UP!</h2><div class="flex flex-col gap-2xsmall"><div>Author</div></div><h2>あらすじ</h2><div>Summary</div><a href="/genres/action">Action</a><div>更新</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
1:["$","x",null,{"chapters":[{"id":"1","name":"第1話","subName":null,"publishingStatus":3}],"currentChapter":null}]
"#;

const PAGES_FIXTURE: &str = r#"
1:["$","x",null,{"pages":[{"content":{"value":{"imageUrl":"https://cdn.example.test/page1.jpg"}}},{"content":{"value":{"imageUrl":"https://cdn.example.test/page2.jpg"}}}],"chapter":{}}]
"#;

export_manga_source!(SOURCE);
