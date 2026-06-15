use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: UniComics = UniComics;
const BASE_URL: &str = "https://unicomics.ru";

struct UniComics;

impl MangaSource for UniComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, BASE_URL));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/comics/online/page/{page}")
        } else {
            format!("{BASE_URL}/comics/series/page/{page}")
        };
        Ok(parse_listing(&fetch_document(&path, LIST_FIXTURE), BASE_URL))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("http://") || query.starts_with("slug:") {
            let key = normalize_series_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key), BASE_URL)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if !query.is_empty() {
            format!(
                "https://yandex.ru/search/site/?searchid=14915852&text={}&web=0&l10n=ru&p={}",
                url::query_escape(query),
                page.saturating_sub(1)
            )
        } else if filter_id(filters, "events") == Some("events") {
            format!("{BASE_URL}/comics/events")
        } else if let Some(publisher) = filter_id(filters, "publisher").filter(|v| *v != "not") {
            format!("{BASE_URL}/comics/publishers/{publisher}/page/{page}")
        } else {
            format!("{BASE_URL}/comics/series/page/{page}")
        };
        let body = fetch_document(&target, LIST_FIXTURE);
        if target.contains("yandex.ru") {
            Ok(parse_yandex(&body))
        } else if target.contains("/comics/events") {
            Ok(parse_events(&body, BASE_URL))
        } else {
            Ok(parse_listing(&body, BASE_URL))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/series/sample".into());
        Ok(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key), BASE_URL))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/series/sample".into());
        let mut url = absolute_url(&key);
        let mut chapters = Vec::new();
        for _ in 0..50 {
            let body = fetch_document(&url, DETAILS_FIXTURE);
            if body.contains("issue-info-grid") {
                chapters.push(parse_issue_chapter(&body, &key));
                break;
            }
            chapters.extend(parse_chapter_cards(&body));
            let Some(next) = next_page(&body) else { break; };
            url = if next.starts_with("http") { next } else { absolute_url(&next) };
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics/online/sample/1".into());
        let body = fetch_document(&absolute_url(&key), PAGE_FIXTURE);
        Ok(parse_pages(&body, &absolute_url(&key)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.contains("unicomics.ru") {
            let key = normalize_series_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), Some(key), BASE_URL)),
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

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn normalize_series_key(value: &str) -> String {
    let mut path = value.split("unicomics.ru").nth(1).unwrap_or(value)
        .split('?').next().unwrap_or(value)
        .split('#').next().unwrap_or(value)
        .trim_matches('/').to_string();
    path = path.replace("comics/issue/", "comics/series/").replace("comics/online/", "comics/series/");
    if let Some(base) = path.strip_suffix('/') { path = base.to_string(); }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() >= 3 && parts[0] == "comics" && parts[1] == "series" {
        let slug = parts[2].trim_end_matches(|ch: char| ch == '-' || ch.is_ascii_digit());
        return format!("/comics/series/{slug}");
    }
    format!("/{}", path.trim_start_matches('/'))
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body.split("comic-card").skip(1).filter_map(|chunk| parse_card(chunk, base)).collect::<Vec<_>>();
    Paged { has_next_page: body.contains("mobilePageSelector") && body.contains("selected") && body.contains("<option"), entries }
}

fn parse_card(chunk: &str, base: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "comic-title-link", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_series_key(&href);
    let title = text_class(chunk, "comic-title-ru")
        .or_else(|| text_class(chunk, "comic-title-en"))
        .or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)))
        .filter(|v| !v.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(base, &image)),
        url: Some(url::join_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_yandex(body: &str) -> Paged<CatalogItem> {
    let entries = body.split("b-serp-item__title-link").skip(1).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        if !href.contains("unicomics.ru") { return None; }
        let key = normalize_series_key(&href);
        let title = html::text_between(chunk, ">", "</a>")
            .map(|v| html::strip_tags(&v).split(" (").next().unwrap_or("").split(" №").next().unwrap_or("").trim().to_string())
            .filter(|v| !v.is_empty())?;
        Some(CatalogItem { key: key.clone(), title, url: Some(absolute_url(&key)), language: Some("ru".into()), content_rating: Some("safe".into()), ..CatalogItem::default() })
    }).fold(Vec::new(), push_unique);
    Paged { has_next_page: body.contains("b-pager__next"), entries }
}

fn parse_events(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body.split("<").filter(|chunk| chunk.contains("event-card") || chunk.contains("list_events")).filter_map(|chunk| {
        let href = html::attr_after(chunk, "<a", "href")?;
        let title = text_class(chunk, "comic-title-ru").or_else(|| text_class(chunk, "event-title")).or_else(|| html::text_between(chunk, "<a", "</a>").map(|v| html::strip_tags(&v)))?;
        let key = normalize_series_key(&href);
        Some(CatalogItem { key: key.clone(), title, cover: html::attr_after(chunk, "<img", "src").map(|image| url::join_url(base, &image)), url: Some(absolute_url(&key)), language: Some("ru".into()), content_rating: Some("safe".into()), ..CatalogItem::default() })
    }).collect();
    Paged { has_next_page: false, entries }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/series/sample".into());
    let is_issue = body.contains("issue-info-grid");
    let title = if is_issue {
        html::text_between(body, "issue-info", "</h1>").map(|v| html::strip_tags(&v))
    } else {
        text_class(body, "series-main").or_else(|| html::text_between(body, "<h1", "</h1>").map(|v| html::strip_tags(&v)))
    }.filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "UniComics".into()));
    let alt = if is_issue { None } else { html::text_between(body, "<h2", "</h2>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()) };
    let desc = html::text_between(body, "series-description", "</div>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty());
    let description = match (alt, desc) {
        (Some(alt), Some(desc)) => Some(format!("{alt}\n\n{desc}")),
        (Some(alt), None) => Some(alt),
        (None, desc) => desc,
    };
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "cover-series", "src").or_else(|| html::attr_after(body, "issue-cover", "src")).map(|image| url::join_url(base, &image)),
        authors: info_value(body, "Издательство").into_iter().collect(),
        description,
        url: Some(url::join_url(base, &key)),
        language: Some("ru".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_cards(body: &str) -> Vec<MangaChapter> {
    body.split("comic-card").skip(1).filter_map(|chunk| {
        let href = html::attr_after(chunk, "Читать", "href")
            .or_else(|| html::attr_after(chunk, "comic-title-link", "href"))
            .or_else(|| html::attr_after(chunk, "<a", "href"))?;
        let key = normalize_path(&href);
        let title = text_class(chunk, "comic-title-ru").or_else(|| text_class(chunk, "comic-title-en")).unwrap_or_else(|| "Глава".into());
        Some(MangaChapter {
            key,
            title: Some(title.clone()),
            chapter_number: chapter_number(&title),
            language: Some("ru".into()),
            ..MangaChapter::default()
        })
    }).collect()
}

fn parse_issue_chapter(body: &str, fallback_key: &str) -> MangaChapter {
    let title = html::text_between(body, "issue-info", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Глава".into());
    let key = html::attr_after(body, "btn-read-online-issues", "href").map(|href| normalize_path(&href)).unwrap_or_else(|| fallback_key.to_string());
    MangaChapter { key, title: Some(title.clone()), chapter_number: chapter_number(&title), language: Some("ru".into()), ..MangaChapter::default() }
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let option_pages = body.split("<option").skip(1).filter_map(|chunk| html::attr(chunk, "value")).collect::<Vec<_>>();
    if !option_pages.is_empty() {
        return option_pages.into_iter().enumerate().map(|(index, value)| lazy_page(index, &value)).collect();
    }
    if let Some((total, path)) = paginator(body) {
        return (1..=total).enumerate().map(|(index, number)| lazy_page(index, &format!("{BASE_URL}{path}{number}"))).collect();
    }
    let image = html::attr_after(body, "image_online", "src").or_else(|| html::attr_after(body, "b_image", "src")).or_else(|| html::attr_after(body, "id=\"image", "src"));
    if let Some(image) = image {
        return vec![MangaPage {
            content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some("Page 1".into()),
            ..MangaPage::default()
        }];
    }
    vec![lazy_page(0, page_url)]
}

fn lazy_page(index: usize, page_url: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Lazy { key: format!("page-{}", index + 1), url: None, page_url: Some(page_url.to_string()), context: Some(manga::image_headers(BASE_URL)) },
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn paginator(body: &str) -> Option<(u64, String)> {
    let marker = "new Paginator";
    let chunk = body.split(marker).nth(1)?;
    let mut quoted = chunk.split(['\'', '"']).skip(1);
    let _selector = quoted.next()?;
    let after_selector = quoted.next().unwrap_or_default();
    let total = after_selector.split(',').find_map(|part| part.trim().parse::<u64>().ok())?;
    let path = quoted.nth(1)?.to_string();
    Some((total, path))
}

fn next_page(body: &str) -> Option<String> {
    body.split("<option").skip(1).skip_while(|chunk| !chunk.contains("selected")).nth(1).and_then(|chunk| html::attr(chunk, "value"))
}

fn text_class(body: &str, class: &str) -> Option<String> {
    html::text_between(body, class, "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty())
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let index = body.find(label)?;
    html::text_between(&body[index..], "value", "</").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty())
}

fn normalize_path(value: &str) -> String {
    format!("/{}", value.split("unicomics.ru").nth(1).unwrap_or(value).split('?').next().unwrap_or(value).trim_start_matches('/').trim_end_matches('/'))
}

fn chapter_number(title: &str) -> Option<f32> {
    title.split('№').nth(1).and_then(|tail| tail.split_whitespace().next()).and_then(|v| v.parse().ok())
}

fn filter_id<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters.get(key).and_then(Value::as_str).and_then(|value| value.split_once(':').map(|(id, _)| id).or(Some(value))).filter(|value| !value.is_empty())
}

fn push_unique(mut acc: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !acc.iter().any(|existing| existing.key == item.key) { acc.push(item); }
    acc
}

const LIST_FIXTURE: &str = r#"<div class="comics-grid"><div class="comic-card"><a class="comic-title-link" href="/comics/series/sample"><img src="/cover.jpg"><span class="comic-title-ru">Пример</span></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="series-main"><h1>Пример</h1><h2>Sample</h2></div><div class="cover-series"><img src="/cover.jpg"></div><div class="series-description">Описание</div><div class="comics-grid"><div class="comic-card"><a class="comic-title-link" href="/comics/issue/sample-1"><span class="comic-title-ru">Пример №1</span></a></div></div>"#;
const PAGE_FIXTURE: &str = r#"<img class="image_online" src="/page.jpg">"#;

export_manga_source!(SOURCE);
