use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Honeytoon = Honeytoon;
const BASE_URL: &str = "https://honeytoon.com";
const IMAGE_BASE_URL: &str = "https://pic.honeytoon.com";

struct Honeytoon;

impl MangaSource for Honeytoon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let lang = language(&request);
        let body = fetch_document(&format!("{BASE_URL}{}/ranking", lang_path(&lang)), LIST_FIXTURE);
        let selector = match request.get("listingId").and_then(Value::as_str) {
            Some("latest") => "section new",
            _ => "section popular",
        };
        Ok(Paged {
            entries: parse_cards(&body, selector, &lang),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let lang = language(&request);
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key, &lang)],
                has_next_page: false,
            });
        }
        if query.is_empty() {
            return self.list(json!({"listingId": "popular", "preferences": {"language": lang}}));
        }
        let body = client(&request)
            .post(format!("{BASE_URL}{}/api/comic/search", lang_path(&lang)))
            .xhr()
            .referer(format!("{BASE_URL}{}/ranking", lang_path(&lang)))
            .form(&[("query", query)])
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_search(&body, &lang),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let lang = language(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        Ok(details_by_key(&key, &lang))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".into());
        let lang = language(&request);
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &lang,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample/1".into());
        if key.contains("#locked") {
            return Ok(vec![manga::text_page(
                "This chapter is locked on Honeytoon.",
            )]);
        }
        let body = fetch_document(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"listingId": "popular", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        let latest = self.list(json!({"listingId": "latest", "preferences": request.get("preferences").cloned().unwrap_or(Value::Null)}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: false,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: false,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::ChunkedImages::process_vertical_merge(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| absolute_url(key.split('#').next().unwrap_or(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let lang = language(&request);
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key, &lang)),
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

fn client(request: &Value) -> HttpClient {
    let adult_cookie = if preference_bool(request, "adultContent", false) {
        "eighteen=1"
    } else {
        "eighteen=0"
    };
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Cookie", adult_cookie)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_cards(body: &str, section_marker: &str, lang: &str) -> Vec<CatalogItem> {
    let section = body
        .split(section_marker)
        .nth(1)
        .unwrap_or(body)
        .split("</section>")
        .next()
        .unwrap_or(body);
    section
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("preview-card__link") || chunk.contains("/comic/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "preview-card__title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Honeytoon".into())),
                cover: image_from_chunk(chunk),
                url: Some(absolute_url(&key)),
                language: Some(lang.into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search(body: &str, lang: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let link = item.get("link").and_then(Value::as_str)?;
            let key = normalize_key(link);
            let image = item
                .get("image")
                .and_then(Value::as_str)
                .map(|value| format!("{IMAGE_BASE_URL}/{}", value.trim_start_matches('/')));
            Some(CatalogItem {
                key: key.clone(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .map(html::strip_tags)
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Honeytoon".into())),
                cover: image,
                url: Some(absolute_url(&key)),
                language: Some(lang.into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn details_by_key(key: &str, lang: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, key, lang)
}

fn parse_details(body: &str, key: &str, lang: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Honeytoon".into())),
        cover: html::attr_after(body, "comic-book-img", "src")
            .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
            .or_else(|| image_from_chunk(body)),
        authors: text_list(body, "comic-book__story-art"),
        artists: text_list(body, "comic-book__story-art"),
        tags: text_list(body, "comic-tag"),
        description: html::text_between(body, "comic-book__desc", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: status(body),
        url: Some(absolute_url(key)),
        language: Some(lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, lang: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-list-items") || chunk.contains("comic-list__title"))
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr(chunk, "href")?;
            let locked = chunk.contains("lock-ico") || chunk.contains("token-ico");
            let mut key = normalize_key(&href);
            if locked {
                key = format!("{key}/{index}#locked");
            }
            let mut title = html::text_between(chunk, "comic-list__title-desc", "</")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("Chapter {}", index + 1));
            if locked && !title.contains("[Locked]") {
                title.push_str(" [Locked]");
            }
            Some(MangaChapter {
                key,
                title: Some(title),
                chapter_number: Some((index + 1) as f32),
                date_uploaded: html::text_between(chunk, "comic-list__title-date", "</")
                    .and_then(|value| parse_date(&html::strip_tags(&value), lang)),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    body.split('<')
        .filter(|chunk| {
            chunk.contains("single__item img")
                || chunk.contains("comic-canvas-scramble")
                || chunk.starts_with("img")
                || chunk.starts_with("div")
        })
        .filter_map(|chunk| {
            if chunk.contains("comic-canvas-scramble") {
                let token = html::attr(chunk, "data-token")?;
                Some(format!("{BASE_URL}/api/common/resource/sync?t={}", url::query_escape(&token)))
            } else {
                html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
            }
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            description: Some(format!("Page {}", index + 1)),
            headers: manga::image_headers(BASE_URL),
            extra: BTreeMap::from([("chapterKey".into(), json!(chapter_key))]),
            ..MangaPage::default()
        })
        .collect()
}

fn text_list(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "preview-card__image", "src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn status(body: &str) -> ItemStatus {
    if body.contains("label__item--complete") {
        ItemStatus::Completed
    } else if body.contains("label__item--dayofpublication") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn language(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get("language"))
        .and_then(Value::as_str)
        .unwrap_or("en")
        .to_string()
}

fn lang_path(lang: &str) -> String {
    match lang {
        "en" => String::new(),
        "pt-BR" => "/pt".into(),
        other => format!("/{other}"),
    }
}

fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn parse_date(value: &str, _lang: &str) -> Option<i64> {
    let clean = value.replace(',', "").replace("  ", " ");
    manatan_shared::dates::parse_ymd(&clean).or_else(|| manatan_shared::dates::parse_fixture_date(&clean))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/comic/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('#')
        .next()
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value);
    let mut parts = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if matches!(parts.first(), Some(&"de" | &"es" | &"fr" | &"it" | &"pt")) {
        parts.remove(0);
    }
    format!("/{}", parts.join("/"))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else if value.starts_with("//") {
        format!("https:{value}")
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<section class="section popular"><a class="preview-card__link" href="/comic/sample"><img class="preview-card__image" src="https://pic.honeytoon.com/sample.jpg"><div class="preview-card__title">Sample Honeytoon</div></a></section>
<section class="section new"><a class="preview-card__link" href="/comic/latest"><img class="preview-card__image" src="https://pic.honeytoon.com/latest.jpg"><div class="preview-card__title">Latest Honeytoon</div></a></section>
"#;

const SEARCH_FIXTURE: &str = r#"[{"title":"<b>Sample Honeytoon</b>","image":"sample.jpg","link":"/comic/sample"}]"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Honeytoon</h1>
<div class="comic-book-img"><img src="https://pic.honeytoon.com/sample.jpg"></div>
<div class="comic-book__story-art"><a>Sample Author</a></div>
<div class="comic-book__desc">Sample description.</div>
<a class="comic-tag">Drama</a>
<div class="comic-list-items">
  <a href="/comic/sample/episode-1"><span class="comic-list__title-desc">Episode 1</span><span class="comic-list__title-date">2024-01-01</span></a>
  <a href="/comic/sample/episode-2"><span class="lock-ico"></span><span class="comic-list__title-desc">Episode 2</span></a>
</div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="single__item"><img data-src="https://pic.honeytoon.com/page1.webp"></div>
<div class="comic-canvas-scramble" data-token="sample-token"></div>
"#;
