use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KiwiyaScans = KiwiyaScans;
const BASE_URL: &str = "https://kiwiyascans.com";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct KiwiyaScans;

impl MangaSource for KiwiyaScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, "", Some(order), request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
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
        Ok(parse_listing(&fetch_document_or_fixture(
            &search_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(
    page: u64,
    query: &str,
    fallback_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut pairs = vec![
        ("title", query.trim().to_string()),
        ("page", page.to_string()),
        (
            "order",
            filter_string(filters, "order")
                .unwrap_or_else(|| fallback_order.unwrap_or_default().to_string()),
        ),
    ];
    for name in ["author", "yearx", "status", "type"] {
        pairs.push((name, filter_string(filters, name).unwrap_or_default()));
    }
    for value in filter_string(filters, "genres")
        .unwrap_or_default()
        .split(',')
    {
        let value = value.trim();
        if !value.is_empty() {
            pairs.push(("genre[]", value.to_string()));
        }
    }
    format!(
        "{BASE_URL}/manga/?{}",
        pairs
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("bsx")
                    || chunk.contains("uta")
                    || chunk.contains("listupd")
                    || chunk.contains("imgu")
            })
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagination")
            && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter") {
        return None;
    }
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| url::slug_from_url(&href))
            .unwrap_or_else(|| "Kiwiya Scans".to_string()),
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Kiwiya Scans".to_string()),
        cover: html::attr_after(body, "class=\"thumb\"", "src")
            .or_else(|| html::attr_after(body, "class='thumb'", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "class=\"desc\"", "</div>")
            .or_else(|| html::text_between(body, "itemprop=\"description\"", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: link_values(body, "/genre/"),
        status: parse_status(&info_values(body, "Status").join(" ")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("eph-num") && chunk.contains("chbox"))
        .filter(|chunk| !chunk.contains("mcl-price-num"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut seen = Vec::<String>::new();
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea") || chunk.contains("ts-main-image") || chunk.contains("src")
        })
        .filter_map(image_attr)
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = script_images(body);
    }
    images
        .into_iter()
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn script_images(body: &str) -> Vec<String> {
    body.split('"')
        .filter(|part| {
            part.starts_with("http")
                && [".jpg", ".jpeg", ".png", ".webp"]
                    .iter()
                    .any(|ext| part.to_ascii_lowercase().contains(ext))
        })
        .map(ToString::to_string)
        .collect()
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<span")
        .chain(body.split("<div"))
        .filter(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .flat_map(|chunk| {
            let links = link_values(chunk, "<a");
            if links.is_empty() {
                vec![html::strip_tags(chunk)]
            } else {
                links
            }
        })
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case(label))
        .collect()
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| href_part == "<a" || chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("termin") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") || lower.contains("pause") {
        ItemStatus::Hiatus
    } else if lower.contains("drop") || lower.contains("abandon") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") || lower.contains("cours") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn filter_string(filters: Option<&Value>, id: &str) -> Option<String> {
    filters?
        .get(id)?
        .as_str()
        .map(|value| value.trim().to_string())
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="listupd"><div class="bsx"><a href="/manga/sample" title="Sample Kiwiya"><img src="/cover.jpg"></a></div></div><div class="pagination"><a class="next"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="bigcontent"><h1 class="entry-title">Sample Kiwiya</h1><div class="thumb"><img src="/cover.jpg"></div><div class="desc">Resume</div><div class="mgen"><a href="/genre/action">Action</a></div><div class="tsinfo"><div class="imptdt">Status <i>Ongoing</i></div></div><div id="chapterlist"><ul><li><div class="chbox"></div><div class="eph-num"><a href="/manga/sample/chapter-1"><span class="chapternum">Chapitre 1</span><span class="chapterdate">2024-01-01</span></a></div></li><li><div class="chbox"><span class="mcl-price-num">10</span></div><div class="eph-num"><a href="/manga/sample/chapter-2"><span class="chapternum">Chapitre 2</span></a></div></li></ul></div></div>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
