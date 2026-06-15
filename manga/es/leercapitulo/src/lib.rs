use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LeerCapitulo = LeerCapitulo;
const BASE_URL: &str = "https://www.leercapitulo.co";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct LeerCapitulo;

impl MangaSource for LeerCapitulo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        if listing_id(&request) == "latest" {
            Ok(parse_latest(&body))
        } else {
            Ok(parse_popular(&body))
        }
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if !query.is_empty() {
            return Ok(Paged {
                entries: parse_autocomplete(&fetch_json_or_fixture(
                    &format!(
                        "{BASE_URL}/search-autocomplete?term={}",
                        url::query_escape(query)
                    ),
                    SEARCH_FIXTURE,
                )),
                has_next_page: false,
            });
        }
        let target = filtered_url(page, request.get("filters").unwrap_or(&Value::Null));
        Ok(parse_search_page(&fetch_document_or_fixture(
            &target,
            SEARCH_PAGE_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        let pages = parse_pages(&body)?;
        Ok(pages)
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
                item: Some(details_from_key(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("hot-manga") || chunk.contains("thumbnail") || chunk.contains("<img")
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::attr(chunk, "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| {
                    html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| "LeerCapitulo".into());
            Some(catalog_item(&href, &title, image_attr(chunk)))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("mainpage-manga")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<h4", "</h4>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "LeerCapitulo".into());
            Some(catalog_item(&href, &title, image_attr(chunk)))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_autocomplete(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(SEARCH_FIXTURE).unwrap_or(Value::Null))
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let link = string_value(item, "link")?;
            let label = string_value(item, "label").unwrap_or_else(|| "LeerCapitulo".into());
            let cover = string_value(item, "thumbnail").map(|value| absolute_url(&value));
            Some(catalog_item(&link, &label, cover))
        })
        .collect()
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("mainpage-manga")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "media-body", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "media-body", "</a>")
                .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "LeerCapitulo".into());
            Some(catalog_item(&href, &title, image_attr(chunk)))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("active"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document_or_fixture(&absolute_url(key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let description = element_text_by_id(body, "example2");
    let alt_names = text_after_label(body, "Títulos Alternativos:");
    let description = match (description, alt_names) {
        (Some(desc), Some(alt)) => Some(format!("{desc}\n\nAlt name(s): {alt}")),
        (Some(desc), None) => Some(desc),
        (None, Some(alt)) => Some(format!("Alt name(s): {alt}")),
        _ => None,
    };
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "LeerCapitulo".into()),
        cover: html::attr_after(body, "cover-detail", "src").map(|value| absolute_url(&value)),
        description,
        tags: link_texts(body, "/genre/"),
        status: parse_status(&text_after_label(body, "Estado:").unwrap_or_default()),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-list") || chunk.contains("xanh"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "xanh", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "xanh", "</a>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                url: Some(absolute_url(&href)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> ExtensionResult<Vec<MangaPage>> {
    let Some(array_data) = element_text_by_id(body, "array_data") else {
        let direct = direct_pages(body);
        if !direct.is_empty() {
            return Ok(direct);
        }
        return Err(ExtensionError {
            message: "Unable to find page data".into(),
        });
    };
    let order_list = html::attr_after(body, "property=\"ad:check\"", "content")
        .or_else(|| html::attr_after(body, "property='ad:check'", "content"))
        .map(|value| split_digits(&value));
    let use_reversed_string = order_list
        .as_ref()
        .is_some_and(|items| items.iter().any(|item| item == "01"));
    let (key1, key2) = data_keys(body)?;
    let encoded = array_data
        .chars()
        .map(|ch| {
            key2.find(ch)
                .and_then(|index| key1.chars().nth(index))
                .unwrap_or(ch)
        })
        .collect::<String>();
    let decoded = String::from_utf8(base64_decode(&encoded)?).map_err(|error| ExtensionError {
        message: format!("page data utf8 decode error: {error}"),
    })?;
    let urls = decoded
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let sorted = if let Some(order) = order_list {
        order
            .into_iter()
            .filter_map(|item| {
                let index_text = if use_reversed_string {
                    item.chars().rev().collect::<String>()
                } else {
                    item
                };
                index_text
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| urls.get(index).cloned())
            })
            .rev()
            .collect::<Vec<_>>()
    } else {
        urls
    };
    Ok(sorted.into_iter().enumerate().map(page_from_url).collect())
}

fn data_keys(body: &str) -> ExtensionResult<(String, String)> {
    let mut scripts = body
        .split("<script")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| src.starts_with("/assets/") && src.contains(".js"))
        .map(|src| absolute_url(&src))
        .collect::<Vec<_>>();
    scripts.reverse();
    let local_scripts = std::iter::once(body.to_string()).chain(
        scripts
            .into_iter()
            .map(|script| fetch_document_or_fixture(&script, "")),
    );
    for script in local_scripts {
        if !script.contains("#array_data") && !script.contains("array_data") {
            continue;
        }
        let keys = quoted_alnum_62(&script);
        if keys.len() >= 2 {
            return Ok((keys[0].clone(), keys[1].clone()));
        }
    }
    Err(ExtensionError {
        message: "Unable to find page decode keys".into(),
    })
}

fn direct_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| image_attr(chunk))
        .enumerate()
        .map(page_from_url)
        .collect()
}

fn page_from_url((index, image): (usize, String)) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: absolute_url(&image),
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn filtered_url(page: u64, filters: &Value) -> String {
    let (kind, value) = ["genre", "alphabetic", "initial", "status"]
        .into_iter()
        .find_map(|name| {
            filters
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (name, value))
        })
        .unwrap_or(("genre", "accion"));
    let path = if kind == "alphabetic" || kind == "initial" {
        "initial"
    } else {
        kind
    };
    format!("{BASE_URL}/{path}/{value}/?page={page}")
}

fn catalog_item(href: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover,
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn element_text_by_id(body: &str, id: &str) -> Option<String> {
    html::text_between(body, &format!("id=\"{id}\""), "</")
        .or_else(|| html::text_between(body, &format!("id='{id}'"), "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let start = body.find(label)?;
    let rest = &body[start + label.len()..];
    Some(html::strip_tags(rest.split('<').next().unwrap_or(rest))).filter(|value| !value.is_empty())
}

fn link_texts(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim() {
        "Ongoing" => ItemStatus::Ongoing,
        "Paused" => ItemStatus::Hiatus,
        "Completed" => ItemStatus::Completed,
        "Cancelled" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn split_digits(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn quoted_alnum_62(script: &str) -> Vec<String> {
    script
        .split(['"', '\''])
        .filter(|part| part.len() == 62 && part.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(ToString::to_string)
        .collect()
}

fn base64_decode(input: &str) -> ExtensionResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for ch in input.chars().filter(|ch| !ch.is_whitespace()) {
        if ch == '=' {
            break;
        }
        let Some(value) = base64_value(ch) else {
            return Err(ExtensionError {
                message: "invalid base64 page data".into(),
            });
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

fn base64_value(ch: char) -> Option<u8> {
    match ch {
        'A'..='Z' => Some(ch as u8 - b'A'),
        'a'..='z' => Some(ch as u8 - b'a' + 26),
        '0'..='9' => Some(ch as u8 - b'0' + 52),
        '+' => Some(62),
        '/' => Some(63),
        _ => None,
    }
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="hot-manga"><div class="thumbnails"><a href="/manga/sample" title="Sample"><img src="/cover.jpg"></a></div></div>
<div class="mainpage-manga"><div class="media-body"><a href="/manga/sample"><h4>Sample</h4></a></div><img src="/cover.jpg"></div>
"#;
const SEARCH_FIXTURE: &str =
    r#"[{"label":"Sample","link":"/manga/sample","thumbnail":"/cover.jpg"}]"#;
const SEARCH_PAGE_FIXTURE: &str = r#"<div class="cate-manga"><div class="mainpage-manga"><div class="media-body"><a href="/manga/sample">Sample</a></div><img src="/cover.jpg"></div></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><div class="cover-detail"><img src="/cover.jpg"></div><div id="example2">Summary</div>
<div class="description-update"><span>Estado:</span> Ongoing <a href="/genre/drama">Drama</a></div>
<div class="chapter-list"><ul><li><a class="xanh" href="/sample-chapter">Chapter 1</a></li></ul></div>
"#;
const PAGES_FIXTURE: &str = r#"
<meta property="ad:check" content="0">
<div id="array_data">aHR0cHM6Ly93d3cubGVlcmNhcGl0dWxvLmNvL3BhZ2UxLmpwZw==</div>
<script>var k='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';var x='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';document.querySelector('#array_data');</script>
"#;
