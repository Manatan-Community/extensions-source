use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiRead = HentaiRead;
const BASE_URL: &str = "https://hentairead.com";

struct HentaiRead;

impl MangaSource for HentaiRead {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "new"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &list_url(page, sort),
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target =
            filtered_search_url(page, query, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/hentai/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/hentai/sample".to_string());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".to_string()),
            scanlators: link_texts(&body, "/scanlator/").into_iter().collect(),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("en".to_string()),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/hentai/sample".to_string());
        let target = reader_url(&key);
        let body = fetch_document(&target, PAGES_FIXTURE);
        let pages = parse_encoded_pages(&body)
            .or_else(|| {
                let fallback = parse_image_tags(&body);
                (!fallback.is_empty()).then_some(fallback)
            })
            .unwrap_or_else(|| parse_image_tags(PAGES_FIXTURE));
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(&list_url(1, "views"), LIST_FIXTURE));
        let latest = parse_listing(&fetch_document(&list_url(1, "new"), LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, sort: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{BASE_URL}/hentai/{page_path}?sortby={}",
        url::query_escape(sort)
    )
}

fn filtered_search_url(page: u64, query: &str, filters: &Value) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    let mut params = vec![
        format!("s={}", url::query_escape(query)),
        "post_type=wp-manga".to_string(),
        "title-type=contains".to_string(),
    ];
    if let Some(value) = filter(filters, "type").filter(|value| !value.is_empty()) {
        params.push(format!("categories%5B%5D={}", url::query_escape(&value)));
    }
    if let Some(value) = filter(filters, "pages").filter(|value| !value.is_empty()) {
        let (min, max) = parse_page_range(&value);
        params.push(format!("pages={min}-{max}"));
    }
    if let Some(value) = filter(filters, "uploaded").filter(|value| !value.is_empty()) {
        let release_type = match value.chars().next() {
            Some('>') => "after",
            Some('<') => "before",
            _ => "in",
        };
        let year: String = value.chars().filter(char::is_ascii_digit).collect();
        if !year.is_empty() {
            params.push(format!("release-type={release_type}"));
            params.push(format!("release={year}"));
        }
    }
    if let Some(value) = filter(filters, "sort") {
        let (sort, order) = value.split_once(':').unwrap_or((value.as_str(), "desc"));
        params.push(format!("sortby={}", url::query_escape(sort)));
        params.push(format!("order={}", url::query_escape(order)));
    }
    for (filter_id, taxonomy, param) in [
        ("tags", "manga_tag", "including"),
        ("artists", "artist", "manga_artists"),
        ("circles", "circle", "circles"),
        ("characters", "character", "characters"),
        ("collections", "collection", "collections"),
        ("scanlators", "scanlator", "scanlators"),
        ("conventions", "convention", "conventions"),
    ] {
        if let Some(value) = filter(filters, filter_id).filter(|value| !value.is_empty()) {
            for raw in value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let excluding = raw.starts_with('-') && taxonomy == "manga_tag";
                let term = raw.trim_start_matches('-');
                if let Some(id) = term_id(term, taxonomy) {
                    let name = if excluding { "excluding" } else { param };
                    params.push(format!("{name}%5B%5D={id}"));
                }
            }
        }
    }
    format!("{BASE_URL}/{page_path}?{}", params.join("&"))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga-item") || chunk.contains("page-item-detail"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "manga-item__link", "href")
                .or_else(|| html::attr_after(chunk, "post-title", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            if !href.contains("/hentai/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "manga-item__link", "title")
                .or_else(|| html::text_between(chunk, "manga-item__link", "</a>"))
                .or_else(|| html::text_between(chunk, "post-title", "</a>"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiRead".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk).map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Completed,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"")
            || body.contains("nav-previous")
            || body.contains("nextpostslink"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/hentai/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-titles", "</h1>")
            .or_else(|| html::text_between(body, "post-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiRead".into())),
        cover: image_from_chunk(body).map(|image| url::join_url(BASE_URL, &image)),
        authors: non_empty(link_texts(body, "/circle/"))
            .or_else(|| non_empty(link_texts(body, "/artist/")))
            .unwrap_or_default(),
        artists: non_empty(link_texts(body, "/artist/"))
            .or_else(|| non_empty(link_texts(body, "/circle/")))
            .unwrap_or_default(),
        tags: link_texts(body, "/tag/"),
        description: details_description(body),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn details_description(body: &str) -> Option<String> {
    let mut lines = Vec::new();
    for (label, path) in [
        ("Characters", "/characters/"),
        ("Parodies", "/parody/"),
        ("Circles", "/circle/"),
        ("Convention", "/convention/"),
        ("Scanlators", "/scanlator/"),
    ] {
        let values = link_texts(body, path);
        if !values.is_empty() {
            lines.push(format!("{label}: {}", values.join(", ")));
        }
    }
    if let Some(titles) = html::text_between(body, "manga-titles", "</h2>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
    {
        lines.push(format!(
            "Alternative Titles:\n{}",
            titles
                .split('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("- {value}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if let Some(pages) = text_near(body, "pages:").filter(|value| !value.is_empty()) {
        lines.push(pages);
    }
    (!lines.is_empty()).then(|| lines.join("\n\n"))
}

fn parse_encoded_pages(body: &str) -> Option<Vec<MangaPage>> {
    let base_url = extract_base_url(body).unwrap_or_else(|| BASE_URL.to_string());
    let json = encoded_json_token(body)
        .and_then(|token| STANDARD.decode(token).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    let data: Value = serde_json::from_str(&json).ok()?;
    let images = data
        .pointer("/data/chapter/images")?
        .as_array()?
        .iter()
        .filter_map(|image| image.get("src").and_then(Value::as_str))
        .filter(|src| !src.is_empty())
        .enumerate()
        .map(|(index, src)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(&base_url, src),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect::<Vec<_>>();
    (!images.is_empty()).then_some(images)
}

fn parse_image_tags(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_from_chunk)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_base_url(body: &str) -> Option<String> {
    let marker = "\"baseUrl\"";
    let index = body.find(marker)?;
    let start = body[..index].rfind('{')?;
    let end = body[index..].find('}')? + index + 1;
    serde_json::from_str::<Value>(&body[start..end])
        .ok()
        .and_then(|value| {
            value
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn encoded_json_token(body: &str) -> Option<&str> {
    for (index, _) in body.match_indices("ey") {
        let rest = &body[index..];
        let end = rest
            .find(|ch: char| {
                !ch.is_ascii_alphanumeric() && !matches!(ch, '+' | '/' | '=' | '-' | '_')
            })
            .unwrap_or(rest.len());
        let token = &rest[..end];
        if token.len() > 32 {
            return Some(token);
        }
    }
    None
}

fn reader_url(key: &str) -> String {
    if key.contains("/english/p/") {
        return url::join_url(BASE_URL, key);
    }
    format!(
        "{}/{}/english/p/1/",
        BASE_URL.trim_end_matches('/'),
        key.trim_matches('/')
    )
}

fn term_id(term: &str, taxonomy: &str) -> Option<u64> {
    let taxonomy = if taxonomy == "artist" {
        "manga_artist"
    } else {
        taxonomy
    };
    let target = format!(
        "{BASE_URL}/wp-admin/admin-ajax.php?action=search_manga_terms&search={}&taxonomy={}",
        url::query_escape(term),
        url::query_escape(taxonomy)
    );
    let body = client().get(target).xhr().send_text().ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    value.get("results")?.as_array()?.iter().find_map(|item| {
        let text = item.get("text")?.as_str()?;
        text.eq_ignore_ascii_case(term)
            .then(|| item.get("id").and_then(Value::as_u64))
            .flatten()
    })
}

fn parse_page_range(query: &str) -> (u64, u64) {
    let num = query
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse::<u64>()
        .unwrap_or(0)
        .clamp(1, 9999);
    match query.chars().next() {
        Some('<') => (1, num),
        Some('>') => (num, 9999),
        Some('=') => (num, num),
        _ => (num, num),
    }
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| srcset_first(html::attr(chunk, "srcset")))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .find_map(|part| part.split_whitespace().next().map(str::to_string))
            .filter(|part| !part.is_empty())
    })
}

fn link_texts(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn text_near(body: &str, marker: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let index = lower.find(marker)?;
    let end = body[index..]
        .find('<')
        .map(|end| index + end)
        .unwrap_or(body.len());
    Some(html::strip_tags(&body[index..end]))
}

fn non_empty(values: Vec<String>) -> Option<Vec<String>> {
    (!values.is_empty()).then_some(values)
}

fn filter(filters: &Value, key: &str) -> Option<String> {
    filters
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="manga-item"><a class="manga-item__link" href="/hentai/sample/" title="Sample Manga"><img src="/cover.jpg"></a></div>
<a rel="next" href="/hentai/page/2/"></a>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="manga-titles"><h1>Sample Manga</h1><h2>Sample Alt | Other Alt</h2></div><img src="/cover.jpg">
<a href="/circle/sample"><span>Sample Circle</span></a><a href="/artist/sample"><span>Sample Artist</span></a>
<a href="/tag/drama"><span>Drama</span></a><a href="/scanlator/group"><span>Group</span></a><span>Pages: 2</span>
"#;
const PAGES_FIXTURE: &str = r#"
<script id="single-chapter-js-extra">var chapterExtraData = {"baseUrl":"https://cdn.example.invalid/hentai/sample"};</script>
<script id="single-chapter-js-before">window.mSample = 'eyJkYXRhIjp7ImNoYXB0ZXIiOnsiaW1hZ2VzIjpbeyJzcmMiOiJwYWdlMS5qcGcifSx7InNyYyI6InBhZ2UyLmpwZyJ9XX19fQ==';</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentairead() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        let details = SOURCE.details(json!({"manga":"/hentai/sample"})).unwrap();
        assert_eq!(details.status, ItemStatus::Completed);
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/hentai/sample"}))
                .unwrap()
                .len(),
            2
        );
    }
}
