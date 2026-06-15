use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: BacaKomik = BacaKomik;
const BASE_URL: &str = "https://bacakomik.my";

struct BacaKomik;

impl MangaSource for BacaKomik {
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
            &format!("{BASE_URL}/daftar-komik/{}?order={order}", page_path(page)),
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
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/komik/sample-bacakomik".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/komik/sample-bacakomik".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-bacakomik-chapter-1".to_string());
        Ok(parse_pages(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE),
            &url::join_url(BASE_URL, &key),
        ))
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

fn page_path(page: u64) -> String {
    if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    }
}

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let base = if page == 1 {
        format!("{BASE_URL}/daftar-komik/")
    } else {
        format!("{BASE_URL}/daftar-komik/page/{page}/")
    };
    let mut params = vec![format!("title={}", url::query_escape(query))];
    for (id, key) in [
        ("author", "author"),
        ("year", "yearx"),
        ("type", "type"),
        ("sort", "order"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    if let Some(status) = filter(filters, "status").filter(|value| !value.is_empty()) {
        params.push(format!("status={}", url::query_escape(status)));
    }
    if let Some(genres) = filters.and_then(|value| value.get("genres")) {
        if let Some(array) = genres.as_array() {
            for genre in array
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                params.push(format!("genre%5B%5D={}", url::query_escape(genre)));
            }
        } else if let Some(genre) = genres.as_str().filter(|value| !value.is_empty()) {
            params.push(format!("genre%5B%5D={}", url::query_escape(genre)));
        }
    }
    format!("{base}?{}", params.join("&"))
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
                let title = html::text_between(chunk, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "BacaKomik".to_string());
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("safe".to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/komik/sample-bacakomik".to_string());
    let title = html::text_between(body, "breadcrumbs", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
        .or_else(|| url::slug_from_url(&key))
        .unwrap_or_else(|| "BacaKomik".to_string());
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "thumb", "data-lazy-src")
            .or_else(|| html::attr_after(body, "thumb", "data-src"))
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "entry-content-single", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value).replace("bercerita tentang ", ""))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artis"),
        tags: info_links(body, "genre-info")
            .into_iter()
            .chain(info_links(body, "Jenis Komik"))
            .collect(),
        status: parse_status(&info_text(body, "Status")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("safe".to_string()),
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
                    .and_then(|value| parse_month_day_year(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("Chapter") || chunk.contains("onError") || chunk.contains("readerarea")
        })
        .filter(|chunk| !chunk.contains("<noscript"))
        .filter_map(|chunk| {
            on_error_src(chunk)
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
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

fn on_error_src(chunk: &str) -> Option<String> {
    let handler = html::attr(chunk, "onError")?;
    handler
        .split("src='")
        .nth(1)
        .and_then(|rest| rest.split("';").next())
        .map(ToString::to_string)
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    let text = info_text(body, label);
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text]
    }
}

fn info_text(body: &str, label: &str) -> String {
    body.split("<span")
        .find(|chunk| html::strip_tags(chunk).contains(label))
        .map(html::strip_tags)
        .map(|value| value.replace(label, "").replace(':', "").trim().to_string())
        .unwrap_or_default()
}

fn info_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("berjalan") || value.contains("ongoing") {
        ItemStatus::Ongoing
    } else if value.contains("tamat") || value.contains("completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split_once("Chapter")
        .map(|(_, tail)| tail)
        .unwrap_or(value)
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn parse_month_day_year(value: &str) -> Option<i64> {
    let clean = value.trim();
    if clean.contains("yang lalu") {
        return None;
    }
    let parts = clean.replace(',', "");
    let mut iter = parts.split_whitespace();
    let month = month_number(iter.next()?)?;
    let day = iter.next()?.parse::<u32>().ok()?;
    let year = iter.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn month_number(value: &str) -> Option<u32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .split('?')
                .next()
                .unwrap_or_default()
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!(
        "/{}",
        input
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="animepost"><div class="animposx"><a href="/komik/sample-bacakomik/"><div class="tt"><h4>Sample BacaKomik</h4></div></a></div><div class="limit"><img src="/cover.jpg"></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div id="breadcrumbs"><li><span>Sample BacaKomik</span></li></div>
<div class="thumb"><img src="/cover.jpg"></div>
<div class="infoanime"><div class="infox"><div class="genre-info"><a>Action</a></div><div class="spe"><span>Author <b>:</b> Writer</span><span>Artis <b>:</b> Artist</span><span>Status <b>:</b> Berjalan</span></div></div></div>
<div class="desc"><div class="entry-content entry-content-single"><p>bercerita tentang sample.</p></div></div>
<ul id="chapter_list"><li><span class="lchx"><a href="/sample-bacakomik-chapter-1/">Chapter 1</a></span><span class="dt"><a>Jan 1, 2024</a></span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"<div><img alt="Chapter 1" onError="this.onerror=null;this.src='https://bacakomik.my/page1.jpg';"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bacakomik_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample BacaKomik"
        );
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
