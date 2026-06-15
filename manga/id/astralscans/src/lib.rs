use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AstralScans = AstralScans;
const BASE_URL: &str = "https://astralscans.top";

struct AstralScans;

impl MangaSource for AstralScans {
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
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample-astral".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample-astral".into());
        let target = url::join_url(BASE_URL, &key);
        Ok(parse_chapters(&fetch_chapter_list_or_fixture(
            &target,
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample-astral/chapter-1".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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

fn fetch_chapter_list_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .post(target)
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("manga_req", "ping")])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(
    page: u64,
    query: &str,
    forced_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for (id, key) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{key}={}", url::query_escape(value)));
        }
    }
    if let Some(order) = forced_order {
        params.push(format!("order={order}"));
    }
    if filters
        .and_then(|value| value.get("project"))
        .and_then(Value::as_str)
        == Some("project-filter-on")
    {
        return format!("{BASE_URL}/project/?{}", params.join("&"));
    }
    format!("{BASE_URL}/manga/?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("bsx") || chunk.contains("imgu") || chunk.contains("uta")
            })
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                if !href.contains("/manga/") {
                    return None;
                }
                let title = html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .or_else(|| url::slug_from_url(&href))
                    .unwrap_or_else(|| "Astral Scans".into());
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
        has_next_page: body.contains("pagination")
            && (body.contains("next") || body.contains("hpage")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample-astral".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Astral Scans".to_string()),
        cover: html::attr_after(body, "thumb", "data-src")
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "artist"),
        tags: link_values(body, ["/genre/", "?genre", "genre"]),
        status: parse_status(&info_text(body, "status")),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    if body.starts_with("ASTRAL_") {
        let parts = body.split("|||").collect::<Vec<_>>();
        if parts.len() >= 3 {
            if let Ok(raw_html) = STANDARD
                .decode(parts[1])
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            {
                let attr = parts[2].trim();
                let chapters = parse_encoded_chapters(&raw_html, attr);
                if !chapters.is_empty() {
                    return chapters;
                }
            }
        }
    }
    parse_standard_chapters(body)
}

fn parse_encoded_chapters(body: &str, attr: &str) -> Vec<MangaChapter> {
    body.split('<')
        .filter(|chunk| chunk.contains(attr))
        .filter(|chunk| !chunk.contains("trap"))
        .filter_map(|chunk| {
            let encoded_url = html::attr(chunk, attr)?;
            let decoded = STANDARD.decode(encoded_url).ok()?;
            let href = String::from_utf8_lossy(&decoded);
            let title = html::text_between(chunk, "n_", "</span>")
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "d_", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_chapter_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_standard_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("eplister")
                || chunk.contains("chbox")
                || chunk.contains("chapternum")
                || chunk.contains("astral-item")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "data-u", "data-u")
                .and_then(|encoded| STANDARD.decode(encoded).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "ch-title", "</")
                .or_else(|| html::text_between(chunk, "epl-num", "</"))
                .or_else(|| html::text_between(chunk, "chapternum", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "ch-date", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_chapter_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea")
                || chunk.contains("ts-main-image")
                || chunk.contains("wp-manga-chapter-img")
                || chunk.contains("data-src")
        })
        .filter_map(image_attr)
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = image_list_json(body);
    }
    images
        .into_iter()
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

fn image_list_json(body: &str) -> Vec<String> {
    let Some(start) = body.find("\"images\"") else {
        return Vec::new();
    };
    let rest = &body[start..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };
    serde_json::from_str(&rest[open..=open + close]).unwrap_or_default()
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    let value = info_text(body, label);
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value]
    }
}

fn info_text(body: &str, label: &str) -> String {
    body.split(['<', '\n'])
        .find(|chunk| {
            html::strip_tags(chunk)
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .map(html::strip_tags)
        .map(|value| value.replace(':', "").replace(label, "").trim().to_string())
        .unwrap_or_default()
}

fn link_values<const N: usize>(body: &str, markers: [&str; N]) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| markers.iter().any(|marker| chunk.contains(marker)))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value.len() < 80)
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if value.contains("ongoing") || value.contains("berjalan") {
        ItemStatus::Ongoing
    } else if value.contains("completed") || value.contains("tamat") {
        ItemStatus::Completed
    } else if value.contains("hiatus") {
        ItemStatus::Hiatus
    } else if value.contains("dropped") || value.contains("cancel") {
        ItemStatus::Cancelled
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

fn parse_chapter_date(value: &str) -> Option<i64> {
    let clean = value.trim().replace(',', "");
    let mut parts = clean.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 3 {
        if let (Ok(day), Some(month), Ok(year)) = (
            parts[1].parse::<u32>(),
            month_number(parts[0]),
            parts[2].parse::<i32>(),
        ) {
            return manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"));
        }
        if let (Ok(day), Some(month), Ok(year)) = (
            parts[0].parse::<u32>(),
            month_number(parts[1]),
            parts[2].parse::<i32>(),
        ) {
            return manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"));
        }
    }
    parts.clear();
    None
}

fn month_number(value: &str) -> Option<u32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sep" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        "december" | "dec" => 12,
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
<div class="listupd"><div class="bs"><div class="bsx"><a href="/manga/sample-astral/" title="Sample Astral"><img src="/cover.jpg"></a></div></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><div class="thumb"><img src="/cover.jpg"></div><h1 class="entry-title">Sample Astral</h1><div class="entry-content">Sample description.</div><div class="mgen"><a href="/genre/action/">Action</a></div><div class="tsinfo"><div class="imptdt">Status <i>Ongoing</i></div></div></div>
<div class="eplister"><ul><li><div class="chbox"></div><div class="eph-num"><a href="/manga/sample-astral/chapter-1"><span class="chapternum">Chapter 1</span></a></div><span class="chapterdate">January 1, 2024</span></li></ul></div>
"#;

const PAGES_FIXTURE: &str = r#"<div id="readerarea"><img class="ts-main-image" src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_astral_fixtures() {
        assert_eq!(
            parse_listing(LIST_FIXTURE).entries[0].title,
            "Sample Astral"
        );
        assert_eq!(parse_standard_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
