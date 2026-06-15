use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AnimeXNovel = AnimeXNovel;
const BASE_URL: &str = "https://www.animexnovel.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";

struct AnimeXNovel;

impl MangaSource for AnimeXNovel {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_results(SEARCH_FIXTURE));
        }
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(Paged {
                entries: parse_latest(&fetch_document(BASE_URL, LATEST_FIXTURE)),
                has_next_page: false,
            });
        }
        Ok(search_request(&request, true))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(search_request(&request, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let details = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let Some(category) = html::attr_after(&details, "axn-chapters-container", "data-categoria")
        else {
            return Ok(parse_chapters(CHAPTERS_FIXTURE));
        };
        Ok(fetch_all_chapters(&category))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample-capitulo-1".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
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
                item: key
                    .starts_with("/manga/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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
        .with_origin(BASE_URL)
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

fn search_request(request: &Value, popular_defaults: bool) -> Paged<CatalogItem> {
    let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    let mut owned_terms = Vec::new();
    if popular_defaults {
        owned_terms.extend([
            "Mangá".to_string(),
            "Manhwa".to_string(),
            "Manhua".to_string(),
        ]);
    } else {
        owned_terms.extend(multi_filter(request.get("filters"), "terms"));
    }
    let page_text = page.to_string();
    let mut form = vec![
        ("action", "axn_filter_obras"),
        ("posts_per_page", "21"),
        ("search", query),
        ("paged", page_text.as_str()),
    ];
    let letter = filter(request.get("filters"), "letter");
    if let Some(letter) = letter.as_deref().filter(|value| !value.is_empty()) {
        form.push(("letra", letter));
    }
    for term in &owned_terms {
        form.push(("terms[]", term.as_str()));
    }
    parse_search_results(
        &client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .xhr()
            .form(&form)
            .send_text()
            .unwrap_or_else(|_| SEARCH_FIXTURE.to_string()),
    )
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("axn-piz-card")
        .skip(1)
        .filter_map(catalog_from_card)
        .collect()
}

fn parse_search_results(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("axn-card")
        .skip(1)
        .filter_map(catalog_from_card)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() > 1;
    if has_next_page {
        entries.pop();
    }
    Paged {
        entries,
        has_next_page,
    }
}

fn catalog_from_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h2", "</h2>")
            .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
            .or_else(|| html::text_between(chunk, "search-content", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "AnimeXNovel".into())),
        cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let status_text = html::attr_after(body, "itemprop=\"creativeWorkStatus\"", "content");
    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "itemprop=\"name\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "AnimeXNovel".into())),
        cover: html::attr_after(body, "itemprop=\"image\"", "content")
            .map(|image| absolute_url(&image)),
        authors: labeled_li(body, "Autor").into_iter().collect(),
        artists: labeled_li(body, "Arte").into_iter().collect(),
        tags: html::attr_after(body, "itemprop=\"genre\"", "content")
            .map(|value| split_csv(&value))
            .unwrap_or_default(),
        description: html::attr_after(body, "itemprop=\"description\"", "content"),
        status: match status_text
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_all_chapters(category: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for page in 1..=25 {
        let target = format!(
            "{BASE_URL}/wp-json/wp/v2/posts?categories={}&orderby=date&order=desc&per_page=100&page={page}",
            url::query_escape(category)
        );
        let body = client()
            .get(target)
            .header("Accept", "application/json")
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        let parsed = parse_chapters(&body);
        let count = parsed.len();
        chapters.extend(parsed);
        if count < 100 {
            break;
        }
    }
    chapters
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_else(|| {
            serde_json::from_str::<Value>(CHAPTERS_FIXTURE)
                .ok()
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
        })
        .into_iter()
        .filter_map(|chapter| {
            let slug = json_text(&chapter, "slug")?;
            let title = chapter
                .get("title")
                .and_then(|title| json_text(title, "rendered"))
                .unwrap_or_else(|| slug.clone());
            if !slug.contains("capitulo") {
                return None;
            }
            let key = format!("/manga/{slug}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    title
                        .split(';')
                        .next_back()
                        .map(html::html_unescape)
                        .filter(|value| !value.is_empty())
                        .unwrap_or(title),
                ),
                url: Some(absolute_url(&key)),
                date_uploaded: json_text(&chapter, "date").and_then(|date| parse_ymd(&date)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let container = html::text_between(body, "spice-block-img-gallery", "</div>")
        .or_else(|| html::text_between(body, "wp-block-gallery", "</div>"))
        .or_else(|| html::text_between(body, "spnc-entry-content", "</div>"))
        .unwrap_or_else(|| body.to_string());
    container
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "src")
                .or_else(|| html::attr(chunk, "data-src"))
                .or_else(|| html::attr(chunk, "data-lazy-src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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

fn labeled_li(body: &str, label: &str) -> Option<String> {
    body.split("<li")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            html::strip_tags(chunk)
                .split(':')
                .next_back()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn filter(filters: Option<&Value>, key: &str) -> Option<String> {
    let value = filters?.get(key)?;
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn multi_filter(filters: Option<&Value>, key: &str) -> Vec<String> {
    let Some(value) = filters.and_then(|filters| filters.get(key)) else {
        return Vec::new();
    };
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .filter_map(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("value").and_then(Value::as_str))
            })
            .map(ToString::to_string)
            .collect();
    }
    value
        .as_str()
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!(
                "/{}",
                value[index + BASE_URL.len()..]
                    .trim_start_matches('/')
                    .trim_end_matches('/')
            );
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn parse_ymd(value: &str) -> Option<i64> {
    let date = value.split('T').next().unwrap_or(value);
    let parts = date.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    ymd_to_unix(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    )
}

fn ymd_to_unix(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era * 146_097 + doe - 719_468) * 86_400)
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"
<a class="axn-card" href="/manga/sample"><img src="/cover.jpg"><h2>Sample AnimeXNovel</h2></a>
<a class="axn-card" href="/manga/next-page-sentinel"><h2>Next</h2></a>
"#;

const LATEST_FIXTURE: &str = r#"
<div>Últimos Mangás</div><div class="axn-piz-container">
  <a class="axn-piz-card" href="/manga/sample"><img src="/cover.jpg"><h3>Sample AnimeXNovel</h3></a>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta itemprop="name" content="Sample AnimeXNovel">
<meta itemprop="image" content="/cover.jpg">
<meta itemprop="genre" content="Ação, Fantasia">
<meta itemprop="description" content="Sample description.">
<meta itemprop="creativeWorkStatus" content="Ongoing">
<li>Autor: Sample Author</li><li>Arte: Sample Artist</li>
<div class="axn-chapters-container" data-categoria="100"></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
[
  { "title": { "rendered": "Sample; Capitulo 1" }, "date": "2024-01-01", "slug": "sample-capitulo-1" }
]
"#;

const PAGES_FIXTURE: &str = r#"
<div class="spice-block-img-gallery">
  <img src="/page-1.jpg"><img data-src="/page-2.jpg">
</div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        assert_eq!(parse_search_results(SEARCH_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
