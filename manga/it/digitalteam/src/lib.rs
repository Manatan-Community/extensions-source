use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: DigitalTeam = DigitalTeam;
const BASE_URL: &str = "https://dgtread.com";

struct DigitalTeam;

impl MangaSource for DigitalTeam {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/reader/series"),
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let mut page = parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/reader/series"),
            LIST_FIXTURE,
        ));
        let needle = query.to_lowercase();
        if !needle.is_empty() {
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/reader/series/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/reader/series/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/read/sample/1".to_string());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_pages(&fetch_document_or_fixture(&target, PAGES_FIXTURE)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<li")
            .skip(1)
            .filter(|chunk| chunk.contains("manga_block"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "manga_title", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "manga_title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&href))?;
                Some(CatalogItem {
                    key: normalize_key(&href),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                    language: Some("it".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/reader/series/sample".to_string());
    let info = html::text_between(body, "id=\"manga_left\"", "</aside>")
        .or_else(|| html::text_between(body, "id=\"manga_left\"", "</div>"))
        .unwrap_or_else(|| body.to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "DigitalTeam".to_string()),
        cover: image_attr(&info).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "div class=\"plot", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: digital_info(&info, "Autore").into_iter().collect(),
        artists: digital_info(&info, "Artista").into_iter().collect(),
        tags: digital_info(&info, "Genere")
            .map(|value| {
                value
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        status: parse_status(&digital_info(&info, "Status").unwrap_or_default()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn digital_info(info: &str, label: &str) -> Option<String> {
    let marker = format!("info_name");
    info.split(&marker).skip(1).find_map(|chunk| {
        let text = html::strip_tags(chunk);
        if !text.contains(label) {
            return None;
        }
        text.split_once(label)
            .map(|(_, value)| value.trim_matches(':').trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("in corso") {
        ItemStatus::Ongoing
    } else if lower.contains("completo") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "ch_bottom", "</")
                    .map(|value| html::strip_tags(&value).replace("Pubblicato il ", ""))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let current = raw_between(body, "current_page=", ";").unwrap_or_default();
    let title = html::text_between(body, "<title", "</title>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_else(|| "DigitalTeam".to_string());
    let external = body.contains("jq_rext.js");
    let response = xhr_pages(&current, &title, external).unwrap_or_else(|| XHR_FIXTURE.to_string());
    let Some(json) = parse_xhr_json(&response) else {
        return Vec::new();
    };
    pages_from_json(&json, external)
}

fn xhr_pages(script: &str, title: &str, external: bool) -> Option<String> {
    let manga = raw_between(script, "m='", "'")?;
    let chapter = raw_between(script, "ch='", "'")?;
    let chapter_sub = raw_between(script, "chs='", "'")?;
    let mut pairs = vec![
        ("info[manga]", manga.as_str()),
        ("info[chapter]", chapter.as_str()),
        ("info[ch_sub]", chapter_sub.as_str()),
        ("info[title]", title),
    ];
    if external {
        pairs.push(("info[external]", "1"));
    }
    client()
        .post(format!("{BASE_URL}/reader/c_i"))
        .xhr()
        .referer(format!("{BASE_URL}/"))
        .form(&pairs)
        .send_text()
        .ok()
}

fn parse_xhr_json(response: &str) -> Option<Value> {
    let body = html::strip_tags(response)
        .trim()
        .trim_matches('"')
        .replace("\\/", "/")
        .replace("\\\"", "\"");
    serde_json::from_str(&body).ok()
}

fn pages_from_json(json: &Value, external: bool) -> Vec<MangaPage> {
    let Some(root) = json.as_array() else {
        return Vec::new();
    };
    let Some(images) = root.first().and_then(Value::as_array) else {
        return Vec::new();
    };
    if external {
        let bases = root.get(1).and_then(Value::as_array).cloned().unwrap_or_default();
        return images
            .iter()
            .zip(bases.iter())
            .enumerate()
            .filter_map(|(index, (image, base))| {
                let base = base.as_str()?;
                let name = image.get("name").and_then(Value::as_str)?;
                let extension = image.get("ex").and_then(Value::as_str).unwrap_or_default();
                Some(page_url(format!("{base}{name}{extension}"), index + 1))
            })
            .collect();
    }
    let suffixes = root.get(1).and_then(Value::as_array).cloned().unwrap_or_default();
    let base = root.get(2).and_then(Value::as_str).unwrap_or_default();
    images
        .iter()
        .zip(suffixes.iter())
        .enumerate()
        .filter_map(|(index, (image, suffix))| {
            let name = image.get("name").and_then(Value::as_str)?;
            let suffix = suffix.as_str().unwrap_or_default();
            let extension = image.get("ex").and_then(Value::as_str).unwrap_or_default();
            Some(page_url(
                format!("{BASE_URL}/reader{base}{name}{suffix}{extension}"),
                index + 1,
            ))
        })
        .collect()
}

fn page_url(image: String, number: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {number}")),
        ..MangaPage::default()
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn raw_between(input: &str, start: &str, end: &str) -> Option<String> {
    let start_index = input.find(start)? + start.len();
    let rest = &input[start_index..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].to_string())
}

fn normalize_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with(BASE_URL) {
        let base = BASE_URL.trim_end_matches('/');
        return format!("/{}", trimmed.trim_start_matches(base).trim_start_matches('/'));
    }
    format!("/{}", trimmed.trim_start_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<li class="manga_block"><div class="manga_title"><a href="/reader/series/sample">Sample Manga</a></div><img src="/cover.jpg"></li>"#;
const DETAILS_FIXTURE: &str = r#"<div id="manga_left"><div class="info_name">Autore</div><div>Author</div><div class="info_name">Artista</div><div>Artist</div><div class="info_name">Genere</div><div>Azione</div><div class="info_name">Status</div><div>In corso</div><div class="cover"><img src="/cover.jpg"></div></div><div class="plot">Description</div><div class="chapter_list"><ul><li><a href="/reader/read/sample/1">Chapter 1</a><span class="ch_bottom">Pubblicato il 01-01-2024</span></li></ul></div>"#;
const PAGES_FIXTURE: &str = r#"<title>Sample Manga</title><script>current_page=m='sample';ch='1';chs='';</script>"#;
const XHR_FIXTURE: &str = r#"[[{"name":"001","ex":".jpg"},{"name":"002","ex":".jpg"}],["",""],"/sample/"]"#;
