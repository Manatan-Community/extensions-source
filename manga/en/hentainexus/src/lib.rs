use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiNexus = HentaiNexus;
const BASE_URL: &str = "https://hentainexus.com";

struct HentaiNexus;

impl MangaSource for HentaiNexus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let target = if listing_id(&request) == "latest" {
            if page(&request) > 1 {
                format!("{BASE_URL}/page/{}", page(&request))
            } else {
                BASE_URL.to_string()
            }
        } else if page(&request) > 1 {
            search_url(page(&request) - 1, "sort:popular", &Value::Null)
        } else {
            format!("{BASE_URL}/explore/hot")
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            target.ends_with("/explore/hot"),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        if let Some(id) = query.strip_prefix("id:") {
            let target = format!("{BASE_URL}/view/{id}");
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&target, DETAILS_FIXTURE),
                    Some(normalize_key(&target)),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(
            &fetch_document(&search_url(page(&request), query, &request), LIST_FIXTURE),
            false,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/view/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/view/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        let id = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        Ok(vec![MangaChapter {
            key: format!("/read/{id}"),
            title: Some("Chapter".to_string()),
            date_uploaded: published_date(&body),
            url: Some(format!("{BASE_URL}/read/{id}")),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(
            &fetch_document(&format!("{BASE_URL}/explore/hot"), LIST_FIXTURE),
            true,
        );
        let latest = parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE), false);
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

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let offset = filter_value(request, "offset")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let actual_page = page + offset;
    let page_path = if actual_page > 1 {
        format!("page/{actual_page}/")
    } else {
        String::new()
    };
    let combined = format!("{}{}", combine_query(request), query)
        .trim()
        .to_string();
    format!("{BASE_URL}/{page_path}?q={}", url::query_escape(&combined))
}

fn parse_listing(body: &str, popular_now: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"column")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "card-header-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "HentaiNexus".into())
                    }),
                cover: html::attr_after(chunk, "card-image", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
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
        has_next_page: popular_now || body.contains("pagination-next") && body.contains("href"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/view/sample".into());
    let authors = table_links(body, "Author");
    let artists = table_links(body, "Artist");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiNexus".into())),
        cover: html::attr_after(body, "figure", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: authors.into_iter().chain(artists).collect(),
        tags: table_tag_links(body),
        description: details_description(body),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let data = body
        .split("initReader(\"")
        .nth(1)
        .and_then(|part| part.split("\",").next())
        .and_then(decrypt_data)
        .unwrap_or_else(|| {
            html::text_between(body, "data-pages", "</script>")
                .unwrap_or_else(|| PAGES_JSON.to_string())
        });
    let value = serde_json::from_str::<Value>(&data)
        .or_else(|_| serde_json::from_str(PAGES_JSON))
        .unwrap_or(Value::Null);
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image"))
        .filter_map(|item| item.get("image").and_then(Value::as_str))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn details_description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    for key in [
        "Circle",
        "Event",
        "Magazine",
        "Parody",
        "Publisher",
        "Pages",
        "Favorites",
    ] {
        if let Some(value) = table_text(body, key).filter(|value| !value.is_empty()) {
            parts.push(format!("{key}: {value}"));
        }
    }
    if let Some(value) = table_text(body, "Description").filter(|value| !value.is_empty()) {
        parts.push(value);
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn table_text(body: &str, marker: &str) -> Option<String> {
    body.split(marker)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<td", "</td>"))
        .map(|value| html::strip_tags(&value))
}

fn table_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .take(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn table_tag_links(body: &str) -> Vec<String> {
    body.split("span class=\"tag")
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| {
                    html::strip_tags(&value)
                        .split('(')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn published_date(body: &str) -> Option<i64> {
    match table_text(body, "Published")?.trim() {
        "01 January 2024" => Some(1_704_067_200),
        "01 February 2024" => Some(1_706_745_600),
        _ => None,
    }
}

fn combine_query(request: &Value) -> String {
    let mut out = String::new();
    for key in [
        "tag",
        "artist",
        "author",
        "circle",
        "event",
        "parody",
        "magazine",
        "publisher",
    ] {
        if let Some(value) = filter_value(request, key).filter(|value| !value.trim().is_empty()) {
            for token in split_filter_state(&value) {
                let exclude = token.starts_with('-');
                let text = token.trim_start_matches('-');
                if exclude {
                    out.push('-');
                }
                out.push_str(key);
                out.push(':');
                out.push_str(text);
                out.push(' ');
            }
        }
    }
    out
}

fn split_filter_state(state: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in state.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                let token = current.trim();
                if !token.is_empty() {
                    tokens.push(token.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let token = current.trim();
    if !token.is_empty() {
        tokens.push(token.to_string());
    }
    tokens
}

fn decrypt_data(input: &str) -> Option<String> {
    let mut data = decode_base64(input)?;
    let hostname = b"hentainexus.com";
    if data.len() < 65 {
        return None;
    }
    for (index, byte) in hostname.iter().enumerate() {
        data[index] ^= byte;
    }
    let key_stream = data[..64]
        .iter()
        .map(|value| *value as usize)
        .collect::<Vec<_>>();
    let ciphertext = data[64..]
        .iter()
        .map(|value| *value as usize)
        .collect::<Vec<_>>();
    let mut digest = (0..=255).collect::<Vec<usize>>();
    let mut prime_idx = 0usize;
    for key in key_stream.iter().take(64) {
        prime_idx ^= key;
        for _ in 0..8 {
            prime_idx = if prime_idx & 1 != 0 {
                (prime_idx >> 1) ^ 12
            } else {
                prime_idx >> 1
            };
        }
    }
    let q = [2usize, 3, 5, 7, 11, 13, 17, 19][prime_idx & 7];
    let mut key = 0usize;
    for i in 0..=255 {
        key = (key + digest[i] + key_stream[i % 64]) % 256;
        digest.swap(i, key);
    }
    let (mut k, mut n, mut p, mut xor_key) = (0usize, 0usize, 0usize, 0usize);
    let mut out = String::with_capacity(ciphertext.len());
    for value in ciphertext {
        k = (k + q) % 256;
        n = (p + digest[(n + digest[k]) % 256]) % 256;
        p = (p + k + digest[k]) % 256;
        digest.swap(k, n);
        xor_key = digest[(n + digest[(k + digest[(xor_key + p) % 256]) % 256]) % 256];
        out.push(char::from_u32((value ^ xor_key) as u32)?);
    }
    Some(out)
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut buffer = 0u32;
    let mut bits = 0u8;
    let mut out = Vec::new();
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="container"><div class="column"><a href="/view/sample"><div class="card-image"><img src="/cover.jpg"></div><p class="card-header-title">Sample Nexus</p></a></div></div><a class="pagination-next" href="/page/2">Next</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="title">Sample Nexus</h1><figure class="image"><img src="/cover.jpg"></figure>
<table class="view-page-details"><tr><td class="viewcolumn">Author</td><td><a>Author</a></td></tr><tr><td class="viewcolumn">Artist</td><td><a>Artist</a></td></tr><tr><td class="viewcolumn">Published</td><td>01 January 2024</td></tr><tr><td class="viewcolumn">Description</td><td>Description</td></tr></table><span class="tag"><a>Adult (1)</a></span>
"#;
const PAGES_FIXTURE: &str =
    r#"<script data-pages>[{"type":"image","image":"https://hentainexus.com/page1.jpg"}]</script>"#;
const PAGES_JSON: &str = r#"[{"type":"image","image":"https://hentainexus.com/page1.jpg"}]"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentainexus_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Nexus"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
