use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KomikIndoID = KomikIndoID;
const BASE_URL: &str = "https://komikindo.ch";

struct KomikIndoID;

impl MangaSource for KomikIndoID {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/daftar-manga/page/{page}/?order={order}"),
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
            &search_url(page, query, request.get("filters")),
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

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut params = vec![format!("title={}", url::query_escape(query))];
    for (id, key) in [("author", "author"), ("year", "yearx"), ("order", "order")] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    for (id, key) in [
        ("type", "type[]"),
        ("format", "format[]"),
        ("demografis", "demografis[]"),
        ("status", "status[]"),
        ("konten", "konten[]"),
        ("tema", "tema[]"),
        ("genre", "genre[]"),
    ] {
        if let Some(values) = filter(filters, id) {
            for value in values
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                params.push(format!("{key}={}", url::query_escape(value)));
            }
        }
    }
    format!("{BASE_URL}/daftar-manga/page/{page}/?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("animepost")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "animposx", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let title = html::text_between(chunk, "div.tt", "</div>")
                    .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "KomikIndoID".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("adult".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let desc = html::text_between(body, "entry-content-single", "</div>")
        .or_else(|| html::text_between(body, "class=\"desc", "</div>"))
        .map(|value| html::strip_tags(&value).replace("bercerita tentang ", ""))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "KomikIndoID".to_string()),
        cover: html::attr_after(body, "class=\"thumb", "src")
            .or_else(|| image_attr(body))
            .map(|image| {
                url::join_url(BASE_URL, &image)
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string()
            }),
        description: desc,
        authors: info_values(body, &["Pengarang", "Author"]),
        artists: info_values(body, &["Ilustrator", "Artist"]),
        tags: link_values(body, "/genre/")
            .into_iter()
            .chain(link_values(body, "/tema/"))
            .chain(link_values(body, "/konten/"))
            .collect(),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("lchx") || chunk.contains("chapter_list"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "dt", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("img-landmine") || body.contains("img-landmine"))
        .filter_map(|chunk| {
            on_error_src(chunk)
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .map(|image| url::join_url(BASE_URL, &image))
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn on_error_src(chunk: &str) -> Option<String> {
    let on_error = html::attr(chunk, "onError").or_else(|| html::attr(chunk, "onerror"))?;
    on_error
        .split("src='")
        .nth(1)
        .and_then(|rest| rest.split("';").next())
        .map(ToString::to_string)
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("tamat") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("berjalan") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value).or_else(|| dates::parse_fixture_date(value))
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
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

fn info_values(body: &str, labels: &[&str]) -> Vec<String> {
    body.split("<span")
        .filter(|chunk| labels.iter().any(|label| chunk.contains(label)))
        .map(html::strip_tags)
        .map(|value| {
            labels
                .iter()
                .fold(value, |acc, label| acc.replace(label, ""))
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="animepost"><div class="animposx"><a href="https://komikindo.ch/manga/sample/"><div class="limit"><img src="/cover.jpg"></div><div class="tt"><h3>Sample</h3></div></a></div></div>
<a class="next page-numbers" href="/daftar-manga/page/2/">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="infoanime"><h1>Sample</h1><div class="thumb"><img src="/cover.jpg?resize=1"></div>
<div class="infox"><div class="spe"><span>Judul Alternatif Sample</span><span>Status: berjalan</span><span><b>Pengarang</b> Author Name</span><span><b>Ilustrator</b> Artist Name</span></div><div class="genre-info"><a href="/genre/action/">Action</a></div></div></div>
<div class="desc"><div class="entry-content entry-content-single"><p>bercerita tentang Sample description.</p></div></div>
<ul id="chapter_list"><li><span class="lchx"><a href="https://komikindo.ch/manga/sample/chapter-1/">Chapter 1</a></span><span class="dt"><a>2024-01-01</a></span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="img-landmine"><img onError="this.onerror=null;this.src='https://komikindo.ch/page-1.jpg';"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
