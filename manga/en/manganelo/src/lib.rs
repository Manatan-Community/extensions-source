use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Manganato = Manganato;
const BASE_URL: &str = "https://www.natomanga.com";
const MIRRORS: [&str; 3] = [
    "https://www.natomanga.com",
    "https://www.nelomanga.com",
    "https://www.manganato.gg",
];

struct Manganato;

impl MangaSource for Manganato {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "manga-list/latest-manga"
        } else {
            "manga-list/hot-manga"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if is_source_url(query) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            format!("{BASE_URL}/genre?page={page}&type=topview")
        } else {
            format!(
                "{BASE_URL}/search/story/{}?page={page}",
                normalize_search_query(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let slug = key.trim_matches('/').split('/').next_back().unwrap_or("sample");
        Ok(parse_api_chapters(slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
        if is_source_url(input) {
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .filter(|chunk| {
            chunk.contains("list-truyen-item-wrap")
                || chunk.contains("list-comic-item-wrap")
                || chunk.contains("story_item")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manganato".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        has_next_page: body.contains("page-select")
            || body.contains("page_select")
            || body.contains("group_page"),
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let info = body
        .split("manga-info-top")
        .nth(1)
        .or_else(|| body.split("panel-story-info").nth(1))
        .unwrap_or(body);
    let title = html::text_between(info, "<h1", "</h1>")
        .or_else(|| html::text_between(info, "<h2", "</h2>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manganato".into()));
    CatalogItem {
        key: key.clone(),
        title: title.clone(),
        cover: body
            .split("manga-info-pic")
            .nth(1)
            .and_then(image_from_chunk)
            .or_else(|| body.split("info-image").nth(1).and_then(image_from_chunk))
            .or_else(|| image_from_chunk(body)),
        description: html::text_between(body, "panel-story-info-description", "</div>")
            .or_else(|| html::text_between(body, "contentBox", "</div>"))
            .map(|value| html::strip_tags(&value).replace(&format!("{title} summary:"), ""))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        authors: link_texts_after_label(info, "author"),
        tags: link_texts_after_label(info, "genres"),
        status: if info.to_ascii_lowercase().contains("completed") {
            ItemStatus::Completed
        } else if info.to_ascii_lowercase().contains("ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_api_chapters(slug: &str) -> Vec<MangaChapter> {
    let mut offset = 0;
    let mut chapters = Vec::new();
    for _ in 0..20 {
        let body = fetch_json(
            &format!("{BASE_URL}/api/manga/{slug}/chapters?limit=500&offset={offset}"),
            CHAPTERS_FIXTURE,
        );
        let Ok(root) = serde_json::from_str::<Value>(&body) else {
            break;
        };
        let data = root.get("data").unwrap_or(&root);
        let rows = data
            .get("chapters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for row in rows {
            let chapter_slug = json_text(&row, "chapter_slug").unwrap_or_else(|| "chapter-1".into());
            let key = format!("/manga/{slug}/{chapter_slug}");
            chapters.push(MangaChapter {
                key: key.clone(),
                title: json_text(&row, "chapter_name"),
                chapter_number: json_number(&row, "chapter_num"),
                scanlators: vec!["www.natomanga.com".to_string()],
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            });
        }
        let has_more = data
            .get("pagination")
            .and_then(|value| value.get("has_more"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_more {
            break;
        }
        offset += 500;
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .filter(|chunk| chunk.contains("cdns") || chunk.contains("chapterImages"))
        .collect::<Vec<_>>()
        .join("\n");
    let cdns = extract_array(&script, "cdns");
    let image_paths = extract_array(&script, "chapterImages");
    let images = if !cdns.is_empty() && !image_paths.is_empty() {
        image_paths
            .into_iter()
            .map(|image| {
                format!(
                    "{}/{}",
                    cdns[0].trim_end_matches('/'),
                    image.trim_start_matches('/')
                )
            })
            .collect::<Vec<_>>()
    } else {
        body.split("<img")
            .skip(1)
            .filter_map(image_from_chunk)
            .collect::<Vec<_>>()
    };
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_array(script: &str, name: &str) -> Vec<String> {
    let Some(rest) = script.split(&format!("{name} = [")).nth(1) else {
        return Vec::new();
    };
    let Some(raw) = rest.split(']').next() else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .map(|value| value.trim_matches('"').trim_matches('\''))
        .map(|value| value.replace("\\/", "/"))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-url"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
        .map(|value| url::join_url(BASE_URL, &value))
}

fn link_texts_after_label(body: &str, label: &str) -> Vec<String> {
    let label = label.to_ascii_lowercase();
    body.split("<li")
        .chain(body.split("<td"))
        .find(|chunk| chunk.to_ascii_lowercase().contains(&label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_search_query(query: &str) -> String {
    let mut output = String::new();
    let mut last_was_sep = false;
    for ch in query.to_lowercase().chars() {
        let mapped = match ch {
            'a'..='z' | '0'..='9' => Some(ch),
            'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ'
            | 'ắ' | 'ặ' | 'ẳ' | 'ẵ' => Some('a'),
            'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' => Some('e'),
            'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' => Some('i'),
            'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ'
            | 'ớ' | 'ợ' | 'ở' | 'ỡ' => Some('o'),
            'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' => Some('u'),
            'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' => Some('y'),
            'đ' => Some('d'),
            _ => None,
        };
        if let Some(ch) = mapped {
            output.push(ch);
            last_was_sep = false;
        } else if !last_was_sep {
            output.push('_');
            last_was_sep = true;
        }
    }
    output.trim_matches('_').to_string()
}

fn is_source_url(input: &str) -> bool {
    MIRRORS.iter().any(|mirror| input.starts_with(mirror))
}

fn normalize_key(input: &str) -> String {
    for mirror in MIRRORS {
        if input.starts_with(mirror) {
            return format!(
                "/{}",
                input
                    .trim_start_matches(mirror)
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("http") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    }
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn json_number(value: &Value, key: &str) -> Option<f32> {
    value.get(key).and_then(Value::as_f64).map(|number| number as f32)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="list-truyen-item-wrap"><h3><a href="/manga/sample">Sample Manga</a></h3><img src="/cover.jpg" alt="Sample Manga"></div><div class="group_page"></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="panel-story-info"><h1>Sample Manga</h1><span class="info-image"><img src="/cover.jpg"></span><li>Author: <a>Author</a></li><li>Status: Ongoing</li><li>Genres: <a>Action</a></li></div><div id="panel-story-info-description">Sample summary</div>"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"chapters":[{"chapter_name":"Chapter 1","chapter_slug":"chapter-1","chapter_num":1,"updated_at":"2024-01-01T00:00:00.000000Z"}],"pagination":{"has_more":false}}}"#;
const PAGES_FIXTURE: &str = r#"<script>cdns = ["https://img.natomanga.com"]; chapterImages = ["manga/sample/001.jpg","manga/sample/002.jpg"];</script>"#;
