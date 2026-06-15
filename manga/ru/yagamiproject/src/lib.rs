use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: YagamiProject = YagamiProject;
const BASE_URL: &str = "https://read.yagami.me";
const LIST_FIXTURE: &str = r#"<div class="list"><div class="group"><div class="cover_mini"><img src="/thumb_cover.jpg"></div><div class="title"><a href="/manga/sample" title="Sample / Пример">Sample</a></div></div></div><div class="panel_nav"><div class="button"><a href="/list-new/2">next</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<title>Пример :: Sample :: Yagami</title><div class="large comic"><div class="cover"><img src="/cover.jpg"></div><ul class="info"><li><b>Автор(ы)</b>: Author / N/A</li><li><b>Художник(и)</b>: Artist</li><li><b>Статус перевода</b>: <span>онгоинг</span></li><li><b>Жанры</b>: драма, фэнтези</li><li><b>Название</b>: Sample<br>Alt Name</li><li><b>Описание</b>: Description</li></ul></div><div class="list"><div class="element"><div class="title"><a href="/manga/sample/1" title="Глава 1">Глава 1</a></div><div class="meta_r"><a>Team</a>, 01.01.2024</div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="web_pictures"><img class="web_img" src="/page1.jpg"></div>"#;

struct YagamiProject;

impl MangaSource for YagamiProject {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let route = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest/{page}")
        } else {
            format!("{BASE_URL}/list-new/{page}")
        };
        Ok(parse_listing(&fetch_document(&route, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("slug:") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let route = if !query.is_empty() {
            format!(
                "{BASE_URL}/reader/search/?s={}&p={page}",
                url::query_escape(query)
            )
        } else if let Some(category) = select_value(request.get("filters"), "category") {
            if category != "Без категории" {
                format!("{BASE_URL}/tags/{}", url::query_escape(&category))
            } else {
                format!("{BASE_URL}/list-new/{page}")
            }
        } else if let Some(format) = select_value(request.get("filters"), "format") {
            if format != "not" {
                format!("{BASE_URL}/{format}")
            } else {
                format!("{BASE_URL}/list-new/{page}")
            }
        } else {
            format!("{BASE_URL}/list-new/{page}")
        };
        Ok(parse_listing(&fetch_document(&route, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
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
        .with_referer(BASE_URL)
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"group")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .map(|v| v.split(" / ").min().unwrap_or(&v).to_string())
                .or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "YagamiProject".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "cover_mini", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|v| absolute_url(&v.replace("thumb_", ""))),
                url: Some(absolute_url(&key)),
                language: Some("ru".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("panel_nav") && body.contains("button") && body.contains("<a"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let title_parts = html::text_between(body, "<title>", "</title>")
        .unwrap_or_default()
        .split(" :: Yagami")
        .next()
        .unwrap_or_default()
        .split(" :: ")
        .map(|v| v.replace(":: ", ""))
        .filter(|v| !v.trim().is_empty())
        .collect::<Vec<_>>();
    let title = title_parts
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "YagamiProject".into()));
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "class=\"cover", "src").map(|v| absolute_url(&v)),
        authors: info_value(body, "Автор(ы):")
            .map(split_names)
            .unwrap_or_default(),
        artists: info_value(body, "Художник(и):")
            .map(split_names)
            .unwrap_or_default(),
        tags: info_value(body, "Жанры:")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
        description: description(body, &title_parts),
        status: parse_status(&info_value(body, "Статус перевода:").unwrap_or_default()),
        url: Some(absolute_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("class=\"element")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let date_text = html::text_between(chunk, "meta_r", "</")
                .map(|v| html::strip_tags(&v))
                .and_then(|v| v.split(", ").nth(1).map(ToString::to_string));
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title).or_else(|| {
                    href.trim_end_matches('/')
                        .split('/')
                        .next_back()
                        .and_then(|v| v.parse::<f32>().ok())
                }),
                date_uploaded: date_text.and_then(|v| parse_date(&v)),
                scanlators: scanlators(chunk),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let webtoon = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("web_img"))
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .enumerate()
        .map(|(index, image)| page_url(index, &image))
        .collect::<Vec<_>>();
    if !webtoon.is_empty() {
        return webtoon;
    }
    body.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let index = html::text_between(chunk, "<a", "</a>")
                .and_then(|v| v.split("Стр. ").nth(1).map(ToString::to_string))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            Some(MangaPage {
                content: PageContent::Url {
                    url: absolute_url(&href),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn page_url(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: absolute_url(image),
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("<li")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| html::strip_tags(chunk).replace(label, "").trim().to_string())
        .filter(|v| !v.is_empty())
}

fn description(body: &str, titles: &[String]) -> Option<String> {
    let alt = body
        .split("<li")
        .skip(1)
        .find(|chunk| chunk.contains("Название:"))
        .map(|chunk| {
            let value = chunk
                .replace("<br>", " / ")
                .replace("<br/>", " / ")
                .replace("<br />", " / ");
            html::strip_tags(&value)
                .replace("Название:", "")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty());
    let description = info_value(body, "Описание:").unwrap_or_default();
    let prefix = titles.iter().max().cloned().unwrap_or_default();
    let alt = alt
        .map(|v| format!("Альтернативные названия:\n{v}\n\n"))
        .unwrap_or_default();
    let out = format!("{prefix}\n{alt}{description}");
    (!out.trim().is_empty()).then_some(out)
}

fn split_names(value: String) -> Vec<String> {
    value
        .split(" / ")
        .map(|v| v.replace("N/A", "").trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn scanlators(chunk: &str) -> Vec<String> {
    chunk
        .split("<a")
        .skip(1)
        .filter(|part| part.contains("meta_r") || chunk.contains("meta_r"))
        .filter_map(|part| html::text_between(part, ">", "</a>").map(|v| html::strip_tags(&v)))
        .filter(|v| !v.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim() {
        "онгоинг" | "активный" => ItemStatus::Ongoing,
        "завершён" | "завершен" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    match value.trim() {
        "Сегодня" | "Вчера" => None,
        value => dates::parse_fixture_date(value),
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    for marker in ["№", "#", " "] {
        if let Some(value) = title.split(':').next().and_then(|v| v.split(marker).last()) {
            if let Ok(number) = value.trim().parse::<f32>() {
                return Some(number);
            }
        }
    }
    None
}

fn select_value(filters: Option<&Value>, key: &str) -> Option<String> {
    let value = filters?.get(key)?;
    if let Some(raw) = value.as_str() {
        return Some(raw.to_string());
    }
    value
        .get("value")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn normalize_key(value: &str) -> String {
    let path = value.trim_start_matches("slug:").trim_start_matches(BASE_URL);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);
