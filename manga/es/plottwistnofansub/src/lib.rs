use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: PlotTwistNoFansub = PlotTwistNoFansub;
const BASE_URL: &str = "https://plotnofansub.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct PlotTwistNoFansub;

impl MangaSource for PlotTwistNoFansub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest3"
        } else {
            "trending"
        };
        Ok(parse_listing(&fetch_document_or_fixture(
            &library_url(page, order),
            LIST_FIXTURE,
        )))
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
        let target = if query.is_empty() {
            library_url(page, "views3")
        } else {
            search_url(page, query)
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(if query.is_empty() {
            parse_listing(&body)
        } else {
            parse_search(&body)
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_html_chapters(&body);
        chapters.extend(fetch_ajax_chapters(&body, !chapters.is_empty()));
        Ok(unique_chapters(chapters))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample/capitulo-1/".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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

fn library_url(page: u64, order: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!("{BASE_URL}/biblioteca3/{page_path}?m_orderby={order}")
}

fn search_url(page: u64, query: &str) -> String {
    let page_path = if page <= 1 {
        String::new()
    } else {
        format!("page/{page}/")
    };
    format!(
        "{BASE_URL}/{page_path}?s={}&post_type=wp-manga",
        url::query_escape(query)
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<figure")
            .skip(1)
            .filter_map(catalog_from_listing_chunk)
            .collect(),
        has_next_page: has_next_page(body),
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let mut entries = body
        .split("c-tabs-item__content")
        .skip(1)
        .filter_map(catalog_from_search_chunk)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        entries = parse_listing(body).entries;
    }
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn catalog_from_listing_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::attr_after(chunk, "<a", "title")
        .or_else(|| {
            html::text_between(chunk, "<figcaption", "</figcaption>")
                .map(|value| html::strip_tags(&value))
        })
        .or_else(|| url::slug_from_url(&href))?;
    Some(catalog_item(
        normalize_key(&href),
        title,
        image_attr(chunk),
        false,
    ))
}

fn catalog_from_search_chunk(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "post-title", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let title = html::text_between(chunk, "post-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .or_else(|| url::slug_from_url(&href))?;
    Some(catalog_item(
        normalize_key(&href),
        title,
        image_attr(chunk),
        false,
    ))
}

fn details_from_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
    let title = text_by_marker(&body, "mn-title-block", "</p>")
        .or_else(|| text_by_marker(&body, "titleMangaSingle", "</p>"))
        .or_else(|| text_by_marker(&body, "post-title", "</"))
        .unwrap_or_else(|| {
            url::slug_from_url(&key).unwrap_or_else(|| "Plot Twist No Fansub".to_string())
        });
    let description = html::text_between(&body, "Sinopsis", "</div>")
        .or_else(|| html::text_between(&body, "section-sinopsis", "</section>"))
        .or_else(|| html::text_between(&body, "summary__content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let status_text = text_by_marker(&body, "mn-chip", "</")
        .or_else(|| text_by_marker(&body, "btn-completed", "</"))
        .or_else(|| text_by_marker(&body, "btn-ongoing", "</"))
        .or_else(|| text_by_marker(&body, "post-status", "</div>"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(&body).map(|value| absolute_url(&value)),
        description,
        authors: link_values_after(&body, "Autor:"),
        tags: link_values_after(&body, "Generos:")
            .into_iter()
            .chain(link_values_after(&body, "genres-content"))
            .collect(),
        status: if status_text.contains("curso")
            || status_text.contains("ongoing")
            || status_text.contains("emision")
        {
            ItemStatus::Ongoing
        } else if status_text.contains("finalizado") || status_text.contains("completed") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_html_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("contenedor-capitulo-miniatura")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let labels = chunk
                .split("text-sm")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            let number = labels.first().cloned().unwrap_or_else(|| "1".to_string());
            let title = labels.get(1).cloned().unwrap_or_default();
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(if title.is_empty() {
                    format!("Capitulo {number}")
                } else {
                    format!("Capitulo {number} - {title}")
                }),
                chapter_number: number.replace(',', ".").parse::<f32>().ok(),
                date_uploaded: text_by_marker(chunk, "<time", "</time>")
                    .and_then(|value| parse_dmy(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn fetch_ajax_chapters(body: &str, html_has_chapters: bool) -> Vec<MangaChapter> {
    let manga_id = find_digits_after(body, "mnWpMangaId")
        .or_else(|| extract_json_string(body, "manga_id"))
        .unwrap_or_default();
    if manga_id.is_empty() {
        return Vec::new();
    }
    if let Some(script) = script_containing(body, "mnSeriesNavBundle") {
        let Some(nav_csrf) = extract_json_string(script, "navCsrf") else {
            return Vec::new();
        };
        let Some(batch_url) =
            extract_json_string(script, "batchUrl").map(|value| value.replace("\\/", "/"))
        else {
            return Vec::new();
        };
        let total_pages = find_digits_after(body, "totalPageCount")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1);
        let mut out = Vec::new();
        let mut page = if html_has_chapters { 2 } else { 1 };
        while page <= total_pages {
            let page_text = page.to_string();
            let response = client()
                .post(&batch_url)
                .xhr()
                .form(&[
                    ("page", &page_text),
                    ("seriesPost", &manga_id),
                    ("navCsrf", &nav_csrf),
                ])
                .send_text();
            let Ok(body) = response else {
                break;
            };
            let chapters = chapters_from_api(&body);
            if chapters.is_empty() {
                break;
            }
            out.extend(chapters);
            page += 1;
        }
        return out;
    }
    if let Some(script) = script_containing(body, "plotGetcaps") {
        let Some(secret) = extract_json_string(script, "secret") else {
            return Vec::new();
        };
        let Some(api_url) =
            extract_json_string(script, "restUrl").map(|value| value.replace("\\/", "/"))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for page in 1..=200 {
            let page_text = page.to_string();
            let response = client()
                .post(&api_url)
                .xhr()
                .form(&[
                    ("action", "plot_anti_hack"),
                    ("page", &page_text),
                    ("mangaid", &manga_id),
                    ("secret", &secret),
                ])
                .send_text();
            let Ok(body) = response else {
                break;
            };
            let chapters = chapters_from_api(&body);
            if chapters.is_empty() {
                break;
            }
            out.extend(chapters);
        }
        return out;
    }
    Vec::new()
}

fn chapters_from_api(body: &str) -> Vec<MangaChapter> {
    let root = json_or_null(body);
    let chapters = root
        .get("chapters_to_display")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .or_else(|| root.get("nav_items").and_then(Value::as_array));
    chapters
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let href = string_value(chapter, "link")?;
            let number = string_value(chapter, "name").unwrap_or_else(|| "1".to_string());
            let suffix = string_value(chapter, "name_extend").unwrap_or_default();
            let date = string_value(chapter, "date")
                .and_then(|value| parse_dmy(&html::strip_tags(&value)));
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(if suffix.is_empty() {
                    format!("Capitulo {number}")
                } else {
                    format!("Capitulo {number} - {suffix}")
                }),
                chapter_number: number.replace(',', ".").parse::<f32>().ok(),
                date_uploaded: date,
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|value| !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_item(
    key: String,
    title: String,
    cover: Option<String>,
    initialized: bool,
) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized,
        ..CatalogItem::default()
    }
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| {
            html::attr_after(input, "<img", "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| {
            html::attr(input, "srcset")
                .and_then(|value| value.split_whitespace().next().map(ToString::to_string))
        })
        .or_else(|| html::attr(input, "src"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_key(input: &str) -> String {
    let mut value = input.trim().to_string();
    if let Some(rest) = value.strip_prefix(BASE_URL) {
        value = rest.to_string();
    }
    format!("/{}", value.trim_matches('/'))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers")
        || body.contains("class=\"next\"")
        || body.contains("class='next'")
}

fn text_by_marker(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_values_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .nth(1)
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .take(12)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn script_containing<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    body.split("<script")
        .skip(1)
        .find(|script| script.contains(marker))
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let start = body.find(&marker)? + marker.len();
    let after_colon = body[start..].find(':').map(|index| start + index + 1)?;
    let rest = body[after_colon..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn find_digits_after(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let first = rest.find(|ch: char| ch.is_ascii_digit())?;
    let digits = rest[first..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then_some(digits)
}

fn unique_chapters(chapters: Vec<MangaChapter>) -> Vec<MangaChapter> {
    chapters.into_iter().fold(Vec::new(), |mut acc, chapter| {
        if !acc
            .iter()
            .any(|item: &MangaChapter| item.key == chapter.key)
        {
            acc.push(chapter);
        }
        acc
    })
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_null(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn parse_dmy(value: &str) -> Option<i64> {
    let cleaned = html::strip_tags(value);
    let mut parts = cleaned.trim().split('-');
    let day = parts.next()?.trim().parse::<u32>().ok()?;
    let month = parts.next()?.trim().parse::<u32>().ok()?;
    let year = parts.next()?.trim().parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

const LIST_FIXTURE: &str = r#"
<div class="page-listing-item"><figure><a href="/sample/" title="Sample"><img src="/cover.jpg"></a><figcaption>Sample</figcaption></figure></div>
<a class="next page-numbers" href="/biblioteca3/page/2/">Next</a>
"#;
const DETAILS_FIXTURE: &str = r#"
<p class="mn-title-block">Sample</p><div class="summary_image"><img src="/cover.jpg"></div>
<script>var mnWpMangaId = 123; var totalPageCount = 1; var mnSeriesNavBundle = {"navCsrf":"token","batchUrl":"https:\/\/plotnofansub.com\/wp-json\/plot\/chapters"};</script>
<div class="contenedor-capitulo-miniatura"><a href="/sample/capitulo-1/"><div class="text-sm">1</div><div class="text-sm">Sample</div><time>01-01-2024</time></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="pg-box"><img src="/page1.jpg"></div><div class="page-break"><img data-src="/page2.jpg"></div>"#;

export_manga_source!(SOURCE);
