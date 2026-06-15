use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AllHentai = AllHentai;
const BASE_URL: &str = "https://20.allhen.online";

struct AllHentai;

impl MangaSource for AllHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "updated"
        } else {
            "rate"
        };
        Ok(parse_listing(&fetch_document(
            &format!(
                "{BASE_URL}/list?sortType={sort}&offset={}",
                50 * (page.saturating_sub(1))
            ),
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
                    &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(
            &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/vol1/1?mtr=true".into());
        Ok(parse_pages(&fetch_document(
            &chapter_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| manga_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&manga_url(&key), DETAILS_FIXTURE),
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
        .with_header("User-Agent", "arora")
        .with_referer(BASE_URL.to_string())
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

fn search_url(page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![format!("offset={}", 50 * page.saturating_sub(1))];
    if !query.is_empty() {
        params.push(format!("q={}", url::query_escape(query)));
    }
    params.push(format!(
        "sortType={}",
        filter_string(filters, "sortType").unwrap_or("RATING")
    ));
    for id in selected_values(filters.get("categories"))
        .into_iter()
        .chain(selected_values(filters.get("genres")))
        .chain(selected_values(filters.get("additional")))
    {
        params.push(format!("{id}=in"));
    }
    format!("{BASE_URL}/search/advancedResults?{}", params.join("&"))
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn selected_values(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter_map(option_id)
            .collect(),
        Some(Value::String(value)) => value.split(',').filter_map(option_id).collect(),
        _ => Vec::new(),
    }
}

fn option_id(value: &str) -> Option<String> {
    let id = value
        .trim()
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or_else(|| value.trim());
    (!id.is_empty()).then(|| id.to_string())
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    let path = path
        .split('?')
        .next()
        .unwrap_or(path)
        .split('#')
        .next()
        .unwrap_or(path);
    let slug = path.trim_matches('/').split('/').next().unwrap_or("sample");
    format!("/{slug}")
}

fn manga_url(key: &str) -> String {
    url::join_url(BASE_URL, key.trim_matches('/'))
}

fn chapter_url(key: &str) -> String {
    let mut key = key.trim_start_matches('/').to_string();
    if !key.contains("?mtr=true") {
        key.push_str("?mtr=true");
    }
    url::join_url(BASE_URL, &key)
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div class=\"tile")
            .skip(1)
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique_item),
        has_next_page: body.contains("nextLink"),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href =
        html::attr_after(chunk, "<h3", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<h3", "title")
        .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|value| html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Манга".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-original")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| value.replace("_p.", "."))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(manga_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    let rating = html::text_between(body, "cr-hero-rating__value", "</")
        .and_then(|value| html::strip_tags(&value).parse::<f32>().ok());
    CatalogItem {
        key: normalize_key(&key),
        title: html::text_between(body, "cr-hero-names__main", "</h1>")
            .or_else(|| html::attr_after(body, "meta[itemprop=name]", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Манга".into())),
        alternate_titles: alt_titles(body),
        cover: html::attr_after(body, "cr-hero-poster__img", "src")
            .or_else(|| html::attr_after(body, "cr-hero-overlay__bg", "data-bg"))
            .map(|value| url::join_url(BASE_URL, &value)),
        authors: person_values(body),
        description: Some(
            [
                rating
                    .map(|value| format!("Rating: {value}"))
                    .unwrap_or_default(),
                html::text_between(body, "cr-description__content", "</div>")
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_default(),
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        ),
        tags: tag_values(body),
        status: parse_status(body),
        rating: rating.map(|value| value / 2.0),
        url: Some(manga_url(&key)),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn alt_titles(body: &str) -> Vec<String> {
    body.split("cr-hero-names__alt")
        .nth(1)
        .unwrap_or_default()
        .split("</h3>")
        .next()
        .unwrap_or_default()
        .split("<span")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "/")
        .collect()
}

fn person_values(body: &str) -> Vec<String> {
    body.split("cr-main-person-item__name")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn tag_values(body: &str) -> Vec<String> {
    body.split("cr-tags__item")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| {
            html::strip_tags(&value)
                .trim_start_matches('#')
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let details = body.to_ascii_lowercase();
    if details.contains("заверш") {
        ItemStatus::Completed
    } else if details.contains("приост") || details.contains("заморож") {
        ItemStatus::Hiatus
    } else if details.contains("продолж") || details.contains("начат") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("item-row") && chunk.contains("chapter-link"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "chapter-link", "href")?;
            let title = html::text_between(chunk, "chapter-link", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Глава".into());
            let number = html::attr(chunk, "data-num")
                .and_then(|value| value.parse::<f32>().ok())
                .map(|value| value / 10.0);
            Some(MangaChapter {
                key: href.split('#').next().unwrap_or(&href).to_string(),
                title: Some(title),
                chapter_number: number,
                date_uploaded: html::attr_after(chunk, "data-date-raw", "data-date-raw")
                    .and_then(|value| parse_date(&value)),
                url: Some(chapter_url(&href)),
                language: Some("ru".into()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: format!("{}/vol1/1", normalize_key(manga_key)),
            title: Some("1".into()),
            chapter_number: Some(1.0),
            url: Some(chapter_url(&format!("{}/vol1/1", normalize_key(manga_key)))),
            language: Some("ru".into()),
            ..MangaChapter::default()
        });
    }
    chapters.reverse();
    chapters
}

fn parse_date(value: &str) -> Option<i64> {
    let date = value.split_whitespace().next()?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<i32>().ok()?;
    let day = parts.next()?.parse::<i32>().ok()?;
    Some(((year - 1970) as i64 * 365 + ((month - 1) * 30 + day - 1) as i64) * 86_400)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let marker = if body.contains("rm_h.readerInit(") {
        "rm_h.readerInit("
    } else {
        "rm_h.readerDoInit("
    };
    let Some(start) = body.find(marker) else {
        return Vec::new();
    };
    let rest = &body[start + marker.len()..];
    let script = rest.split(");").next().unwrap_or(rest);
    script
        .split("],")
        .filter_map(page_url_from_reader_entry)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.clone(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn page_url_from_reader_entry(entry: &str) -> Option<String> {
    let strings = quoted_strings(entry);
    if strings.len() < 3 {
        return None;
    }
    let (first, second, third) = (&strings[0], &strings[1], &strings[2]);
    let mut image = if second.is_empty() && third.starts_with("/static/") {
        format!("{BASE_URL}{third}")
    } else if second.ends_with("/manga/") {
        format!("{first}{third}")
    } else {
        format!("{second}{first}{third}")
    };
    if !image.contains("://") {
        image = format!("https:{image}");
    }
    if image.contains("one-way.work") {
        image = image.split('?').next().unwrap_or(&image).to_string();
    }
    Some(image.replace("//resh", "//h"))
}

fn quoted_strings(input: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\'' && ch != '"' {
            continue;
        }
        let quote = ch;
        let mut value = String::new();
        while let Some(next) = chars.next() {
            if next == quote {
                break;
            }
            value.push(next);
        }
        values.push(html::html_unescape(&value.replace("\\/", "/")));
    }
    values
}

fn push_unique_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<span class="pagination"><a class="nextLink"></a></span><div class="tile"><img data-original="/cover_p.jpg"><div class="desc"><h3><a href="/sample" title="Sample">Sample</a></h3></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="cr-hero-names__main">Sample</h1><img class="cr-hero-poster__img" src="/cover.jpg"><div class="cr-tags"><a class="cr-tags__item"><span>драма</span></a></div><tr class="item-row" data-num="10"><td class="item-title"><a href="/sample/vol1/1" class="chapter-link">1 - 1</a></td><td class="date" data-date-raw="2024-01-01 00:00:00"></td></tr>
"#;
const PAGES_FIXTURE: &str = r#"
<script>rm_h.readerInit({}, [['','',"/static/page.jpg",600,800]], false, [], false);</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_allhentai_flow() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].key, "/sample");
        let chapters = SOURCE.chapters(json!({"manga":{"key":"/sample"}})).unwrap();
        assert_eq!(chapters[0].key, "/sample/vol1/1");
        let pages = SOURCE
            .pages(json!({"chapter":{"key":chapters[0].key.clone()}}))
            .unwrap();
        assert_eq!(pages[0].description.as_deref(), Some("Page 1"));
    }
}
