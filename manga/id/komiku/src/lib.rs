use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Komiku = Komiku;
const BASE_URL: &str = "https://komiku.org";
const API_URL: &str = "https://api.komiku.org";

struct Komiku;

impl MangaSource for Komiku {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "modified"
        } else {
            "meta_value_num"
        };
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &api_url(page, "", Some(order), request.get("filters")),
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
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &api_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_matches('/'));
    }
    format!("/{}", value.trim_matches('/'))
}

fn api_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut path = format!("{API_URL}/manga");
    if page > 1 {
        path.push_str(&format!("/page/{page}"));
    }
    let mut params = Vec::new();
    if !query.is_empty() {
        params.push(format!("s={}", url::query_escape(query)));
    }
    for id in ["tipe", "genre", "genre2", "statusmanga"] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{id}={}", url::query_escape(value)));
        }
    }
    let selected_order = filter(filters, "orderby")
        .filter(|value| !value.is_empty())
        .or(order);
    if let Some(value) = selected_order {
        params.push(format!("orderby={}", url::query_escape(value)));
    }
    if params.is_empty() {
        path
    } else {
        format!("{path}?{}", params.join("&"))
    }
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("bge"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Komiku".to_string());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| without_query(&absolute_url(&image))),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_catalog_item);
    Paged {
        has_next_page: body.contains("hx-get") || entries.len() >= 10,
        entries,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let description = html::text_between(body, "id=\"Sinopsis\"", "</div>")
        .or_else(|| html::text_between(body, "#Sinopsis", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let indonesian_title = info_text(body, "Judul Indonesia");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Komiku".to_string())),
        cover: html::attr_after(body, "class=\"ims", "src")
            .or_else(|| image_attr(body))
            .map(|image| without_query(&absolute_url(&image))),
        description: match (description, indonesian_title) {
            (Some(desc), Some(title)) if !title.is_empty() => {
                Some(format!("{desc}\n\nJudul Indonesia: {title}"))
            }
            (desc, _) => desc,
        },
        authors: info_text(body, "Pengarang")
            .or_else(|| info_text(body, "Komikus"))
            .into_iter()
            .collect(),
        tags: link_values(body, "ul.genre")
            .into_iter()
            .chain(link_values(body, "/genre/"))
            .collect(),
        status: parse_status(&info_text(body, "Status").unwrap_or_else(|| body.to_string())),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("judulseries"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "tanggalseries", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let section =
        html::text_between(body, "id=\"Baca_Komik\"", "</div>").unwrap_or_else(|| body.to_string());
    section
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "src"))
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("<tr")
        .chain(body.split("<td"))
        .find_map(|chunk| {
            if !chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
            {
                return None;
            }
            let text = html::strip_tags(chunk);
            text.split(label)
                .nth(1)
                .map(|value| value.trim_matches([':', ' ']).trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker) || chunk.contains("genre"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("ongoing") || lower.contains("on going") {
        ItemStatus::Ongoing
    } else if lower.contains("end") || lower.contains("completed") || lower.contains("tamat") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    if value.contains("lalu") {
        return None;
    }
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn without_query(value: &str) -> String {
    value.split('?').next().unwrap_or(value).to_string()
}

fn push_unique_catalog_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bge"><a href="/manga/sample"><h3>Sample Manga</h3><img src="/cover.jpg?x=1"></a></div><span hx-get="/manga/page/2"></span>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Manga</h1><div class="ims"><img src="/cover.jpg"></div><div id="Sinopsis"><p>Sample description.</p></div><table class="inftable"><tr><td>Status</td><td>Ongoing</td></tr><tr><td>Pengarang</td><td>Author</td></tr></table><ul class="genre"><li class="genre"><a href="/genre/action"><span>Action</span></a></li></ul><table id="Daftar_Chapter"><tr><td class="judulseries"><a href="/manga/sample/chapter-1">Chapter 1</a></td><td class="tanggalseries">01/01/2024</td></tr></table>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="Baca_Komik"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
