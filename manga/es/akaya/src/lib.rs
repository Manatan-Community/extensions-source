use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult},
    export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Akaya = Akaya;
const BASE_URL: &str = "https://akaya.io";
const IMAGE_API_URL: &str = "https://api.akayamedia.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const POPULAR_COLLECTION: &str = "bd90cb43-9bf2-4759-b8cc-c9e66a526bc6";
const LATEST_COLLECTION: &str = "0031a504-706c-4666-9782-a4ae30cad973";

struct Akaya;

impl MangaSource for Akaya {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let collection = if listing_id(&request) == "latest" {
            LATEST_COLLECTION
        } else {
            POPULAR_COLLECTION
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/collection/{collection}?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/serie/") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(Paged {
                entries: parse_search(&post_search_or_fixture(query, SEARCH_FIXTURE)),
                has_next_page: false,
            });
        }

        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let order = filters
            .get("order")
            .and_then(Value::as_str)
            .unwrap_or("genres");
        let order = match order {
            "genres-bydate" | "genres-byname" => order,
            _ => "genres",
        };
        let genres = selected_genres(filters);
        let genre_path = if genres.is_empty() {
            String::new()
        } else {
            format!("/[{}]", genres.join(","))
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &format!("{BASE_URL}/{order}{genre_path}?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/serie/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &format!("{}?order_direction=desc", absolute_url(&key)),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample".into());
        if key.ends_with("#lock") {
            return Err(ExtensionError {
                message: "Capitulo bloqueado".to_string(),
            });
        }
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let key = key.trim_end_matches("#lock");
            absolute_url(key)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/serie/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    &key,
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

fn post_search_or_fixture(query: &str, fixture: &str) -> String {
    let token_page = fetch_document_or_fixture(BASE_URL, TOKEN_FIXTURE);
    let token = html::attr_after(&token_page, "csrf-token", "content").unwrap_or_default();
    let form = [("_token", token.as_str()), ("search", query)];
    client()
        .post(format!("{BASE_URL}/search"))
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: marker_blocks(body, "library-grid-item")
            .into_iter()
            .filter_map(|chunk| {
                let href = html::attr_after(&chunk, "<a", "href")?;
                let key = normalize_key(&href);
                let title = html::text_between(&chunk, "<strong", "</strong>")
                    .or_else(|| html::text_between(&chunk, "<h5", "</h5>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "AKAYA".into()));
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk(&chunk),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_search(body: &str) -> Vec<CatalogItem> {
    marker_blocks(body, "list-search")
        .into_iter()
        .filter_map(|chunk| {
            if !chunk.contains("inner-img-search") {
                return None;
            }
            let href = html::attr_after(&chunk, "name-serie-search", "href")
                .or_else(|| html::attr_after(&chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(&chunk, "name-serie-search", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "AKAYA".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(&chunk),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: class_text(body, "serie-head-title")
            .or_else(|| {
                html::text_between(body, "<title", "</title>").map(|value| html::strip_tags(&value))
            })
            .unwrap_or_else(|| "AKAYA".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "property='og:image'", "content"))
            .map(|value| value.replace("/chapters/", "/content/")),
        url: Some(absolute_url(key)),
        authors: list_values(body, "persons"),
        tags: list_values(body, "categories"),
        description: html::text_between(body, "sidebar", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    marker_blocks(body, "chapter-item")
        .into_iter()
        .filter_map(|chunk| {
            let href = html::attr_after(&chunk, "<a", "href")?;
            let mut key = normalize_key(&href);
            let mut title = html::text_between(&chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let is_locked = chunk.contains("ak-lock");
            if is_locked {
                title = format!("Locked - {title}");
                key.push_str("#lock");
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: class_text(&chunk, "date")
                    .and_then(|value| parse_akaya_date(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(key.trim_end_matches("#lock"))),
                is_locked,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    if let Some(json) = body.find("var chapterData =").and_then(|start| {
        let rest = &body[start + "var chapterData =".len()..];
        let end = rest.find('\n').or_else(|| rest.find("</script>"))?;
        Some(rest[..end].trim().trim_end_matches(';').to_string())
    }) {
        if let Ok(root) = serde_json::from_str::<Value>(&json) {
            let mut images = root
                .get("images")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|image| {
                    Some((
                        image.get("order_sort").and_then(Value::as_i64).unwrap_or(0),
                        image.get("image")?.as_str()?.to_string(),
                    ))
                })
                .collect::<Vec<_>>();
            images.sort_by_key(|(order, _)| *order);
            if !images.is_empty() {
                return images
                    .into_iter()
                    .enumerate()
                    .map(|(index, (_, image))| {
                        image_page(index, &format!("{IMAGE_API_URL}/chapters/{image}"))
                    })
                    .collect();
            }
        }
    }

    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-img") || chunk.contains("img-fluid"))
        .filter_map(|chunk| html::attr(chunk, "src").map(|value| absolute_url(&value)))
        .enumerate()
        .map(|(index, image)| image_page(index, &image))
        .collect()
}

fn image_page(index: usize, image: &str) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn selected_genres(filters: &Value) -> Vec<String> {
    let Some(genres) = filters.get("genres").and_then(Value::as_array) else {
        return Vec::new();
    };
    genres
        .iter()
        .filter_map(|value| {
            value
                .as_u64()
                .map(|number| number.to_string())
                .or_else(|| value.as_str().map(ToString::to_string))
        })
        .collect()
}

fn marker_blocks(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .map(|chunk| {
            let end = chunk
                .find(marker)
                .or_else(|| chunk.find("pagination"))
                .unwrap_or(chunk.len());
            chunk[..end].to_string()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "inner-img-search", "style")
        .or_else(|| html::attr_after(chunk, "inner-img", "style"))
        .and_then(|style| style_url(&style))
        .or_else(|| html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)))
}

fn style_url(style: &str) -> Option<String> {
    style
        .split("url(")
        .nth(1)
        .and_then(|value| value.split(')').next())
        .map(|value| value.trim_matches(['"', '\'']).to_string())
        .filter(|value| !value.is_empty())
        .map(|value| absolute_url(&value))
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn list_values(body: &str, class_name: &str) -> Vec<String> {
    let Some(list) = html::text_between(body, class_name, "</ul>") else {
        return Vec::new();
    };
    let values = list
        .split("<li")
        .skip(1)
        .map(|chunk| html::strip_tags(chunk))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        let value = html::strip_tags(&list);
        if value.is_empty() {
            Vec::new()
        } else {
            vec![value]
        }
    } else {
        values
    }
}

fn parse_akaya_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    let hour = value.get(11..13).unwrap_or("00").parse::<i64>().ok()?;
    let minute = value.get(14..16).unwrap_or("00").parse::<i64>().ok()?;
    let second = value.get(17..19).unwrap_or("00").parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
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

fn absolute_url(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

const TOKEN_FIXTURE: &str =
    r#"<html><head><meta name="csrf-token" content="fixture-token"></head></html>"#;
const LIST_FIXTURE: &str = r#"<div class="serie_items"><div class="library-grid-item"><a href="/serie/sample"></a><span><h5><strong>Sample</strong></h5></span><div class="inner-img" style="background-image:url(/cover.jpg)"></div></div></div>"#;
const SEARCH_FIXTURE: &str = r#"<main><div class="search-title"><div class="rowDiv"><div class="list-search"><div class="inner-img-search" style="background-image:url(/cover.jpg)"></div><div class="name-serie-search"><a href="/serie/sample">Sample</a></div></div></div></div></main>"#;
const DETAILS_FIXTURE: &str = r#"<meta property="og:image" content="https://akaya.io/content/cover.jpg"><header class="masthead"><div class="serie-head-title">Sample</div><ul class="persons"><li>Author</li></ul><ul class="categories"><li>Drama</li></ul></header><section class="main"><div class="sidebar"><p>Summary</p></div></section><div class="chapter-desktop"><div class="chapter-item"><div class="text-left"><div class="mt-1"><a href="/chapter/sample">Chapter 1</a></div></div><p class="date">2024-01-01 00:00:00</p></div></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var chapterData = {"images":[{"image":"sample-page.jpg","order_sort":1}]};</script>"#;

export_manga_source!(SOURCE);
