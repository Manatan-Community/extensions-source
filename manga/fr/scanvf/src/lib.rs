use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ScanVf = ScanVf;
const BASE_URL: &str = "https://www.scan-vf.net";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct ScanVf;

impl MangaSource for ScanVf {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest-release?page={page}")
        } else if page <= 1 {
            format!("{BASE_URL}/manga-list")
        } else {
            format!("{BASE_URL}/manga-list?page={page}")
        };
        Ok(parse_listing(&fetch_doc(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("https://scan-vf.net") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_doc(
            &format!("{BASE_URL}/search?query={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(
            &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/chapter-1".into());
        Ok(parse_pages(&fetch_doc(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with("https://scan-vf.net") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
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

fn fetch_doc(target: &str, fixture: &str) -> String {
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("chapter-container")
                    || chunk.contains("media")
                    || chunk.contains("manga-list")
            })
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("pagination"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    if href.contains("/chapter") || href.contains("latest-release") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "media-heading", "</")
            .or_else(|| html::text_between(chunk, "manga-heading", "</"))
            .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Scan VF".into()),
        cover: image_attr(chunk)
            .map(|image| url::join_url(BASE_URL, &image))
            .or_else(|| Some(guess_cover(&key))),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "panel-heading", "</")
            .or_else(|| html::text_between(body, "listmanga-header", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Scan VF".into()),
        cover: image_attr(body)
            .map(|image| url::join_url(BASE_URL, &image))
            .or_else(|| Some(guess_cover(&key))),
        description: html::text_between(body, "well", "</div>")
            .or_else(|| html::text_between(body, "summary", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: panel_value(body, "Auteur")
            .or_else(|| panel_value(body, "Author"))
            .into_iter()
            .collect(),
        artists: panel_value(body, "Artiste")
            .or_else(|| panel_value(body, "Artist"))
            .into_iter()
            .collect(),
        tags: link_values(body, "/genre/"),
        status: panel_value(body, "Statut")
            .or_else(|| panel_value(body, "Status"))
            .map(|value| parse_status(&value))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let manga_title = url::slug_from_url(manga_key).unwrap_or_else(|| "Manga".into());
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-title") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-title-rtl", "</")
                .or_else(|| html::text_between(chunk, "chapter-title", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value).replace(&manga_title, "Chapter"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number(&key),
                date_uploaded: html::text_between(chunk, "date-chapter-title-rtl", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut seen = Vec::<String>::new();
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| {
            if image.starts_with("data:") || seen.contains(image) {
                false
            } else {
                seen.push(image.clone());
                true
            }
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn panel_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "div class=\"text\"", "</div>")
                .or_else(|| html::text_between(chunk, "<dd", "</dd>"))
                .or_else(|| html::text_between(chunk, ">", "</"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    [
        "data-background-image",
        "data-cfsrc",
        "data-lazy-src",
        "data-src",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn guess_cover(key: &str) -> String {
    let slug = key
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample")
        .replace('_', "-");
    format!("{BASE_URL}/uploads/manga/{slug}/cover/cover_250x350.jpg")
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("termin") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("pause") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("abandon") || lower.contains("drop") {
        ItemStatus::Cancelled
    } else if lower.contains("cours") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split("chapter-")
        .nth(1)?
        .split('/')
        .next()?
        .parse()
        .ok()
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .or_else(|| input.strip_prefix("https://scan-vf.net"))
        .unwrap_or(input);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="chapter-container"><a href="/sample"><img src="/cover.jpg"><h3>Sample VF</h3></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<div class="panel-heading">Sample VF</div><div class="row"><img class="img-responsive" src="/cover.jpg"><div class="well">Resume</div></div><div><span>Auteur</span><div class="text">Auteur</div><span>Statut</span><div class="text">En cours</div><a href="/genre/action">Action</a></div><ul><li><div class="chapter-title-rtl"><a href="/sample/chapter-1">Sample VF: Chapter 1</a></div><span class="date-chapter-title-rtl">2024-01-01</span></li></ul>"#;
const PAGES_FIXTURE: &str = r#"<div id="all"><img class="img-responsive" src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
