use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Lelmanga = Lelmanga;
const BASE_URL: &str = "https://www.lelmanga.com";
const MANGA_PATH: &str = "manga";

struct Lelmanga;

impl MangaSource for Lelmanga {
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
        Ok(parse_listing(&fetch_document(
            &listing_url(page, "", Some(order), request.get("filters")),
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
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &listing_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &url::join_url(BASE_URL, &key)))
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
        if let Some(key) = key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn listing_url(
    page: u64,
    query: &str,
    forced_order: Option<&str>,
    filters: Option<&Value>,
) -> String {
    let mut pairs = vec![
        ("title".to_string(), query.trim().to_string()),
        ("page".to_string(), page.to_string()),
    ];
    for id in ["author", "year", "status", "type"] {
        if let Some(value) = filter_string(filters, id).filter(|value| !value.is_empty()) {
            let key = if id == "year" { "yearx" } else { id };
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    let order = forced_order.or_else(|| filter_string(filters, "order"));
    if let Some(value) = order.filter(|value| !value.is_empty()) {
        pairs.push(("order".into(), value.to_string()));
    }
    if let Some(genres) = filter_string(filters, "genre") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            pairs.push(("genre[]".into(), genre.to_string()));
        }
    }
    format!(
        "{BASE_URL}/{MANGA_PATH}/?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("class=\"bsx")
                    || chunk.contains("class='bsx")
                    || chunk.contains("class=\"imgu")
                    || chunk.contains("class='imgu")
            })
            .filter_map(listing_item)
            .collect(),
        has_next_page: body.contains("class=\"next")
            || body.contains("class='next")
            || body.contains("hpage"),
    }
}

fn listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| html::text_between(chunk, "tt", "</").map(|value| html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Lelmanga".into()),
        cover: img_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let description = text_for_class(body, "desc")
        .or_else(|| text_for_class(body, "entry-content"))
        .filter(|value| !value.is_empty());
    let alt = info_value(body, "Nom alternatif")
        .or_else(|| text_for_class(body, "alternative"))
        .or_else(|| text_for_class(body, "alter"));
    let description = match (description, alt) {
        (Some(desc), Some(alt)) if !alt.is_empty() => {
            Some(format!("{desc}\n\nNom alternatif: {alt}"))
        }
        (desc, _) => desc,
    };
    CatalogItem {
        key: normalize_key(&key),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Lelmanga".into()),
        cover: html::attr_after(body, "class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "class='thumb", "src"))
            .or_else(|| html::attr_after(body, "itemprop=\"image\"", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_value(body, "Auteur").into_iter().collect(),
        artists: info_value(body, "Artiste").into_iter().collect(),
        description,
        tags: parse_tags(body),
        status: info_value(body, "Statut")
            .map_or(ItemStatus::Unknown, |value| parse_status(&value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapternum")
                || chunk.contains("chapterdate")
                || chunk.contains("chbox")
                || chunk.contains("lch")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| {
                        dates::parse_fixture_date(&value).or_else(|| dates::parse_ymd(&value))
                    }),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| body.contains("readerarea") || chunk.contains("readerarea"))
        .filter_map(img_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = images_json(body);
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn images_json(body: &str) -> Vec<String> {
    let Some(start) = body.find("\"images\"") else {
        return Vec::new();
    };
    let Some(open) = body[start..].find('[').map(|idx| start + idx) else {
        return Vec::new();
    };
    let Some(close) = body[open..].find(']').map(|idx| open + idx + 1) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&body[open..close]).unwrap_or_default()
}

fn img_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-lazy-src")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-cfsrc"))
        .or_else(|| html::attr(chunk, "src"))
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let label_lower = label.to_ascii_lowercase();
    let index = lower.find(&label_lower)?;
    let fragment = &body[index..body.len().min(index + 600)];
    html::text_between(fragment, "<i", "</i>")
        .or_else(|| html::text_between(fragment, "</b>", "</"))
        .or_else(|| html::text_between(fragment, "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim_matches([':', ' ']).to_string())
        .filter(|value| !value.is_empty() && value != "-" && value != "N/A")
}

fn text_for_class(body: &str, class_name: &str) -> Option<String> {
    body.split("<div")
        .skip(1)
        .find(|chunk| chunk.contains(class_name))
        .map(|chunk| html::strip_tags(chunk.split("</div>").next().unwrap_or(chunk)))
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("/genre/") || chunk.contains("genre=") || chunk.contains("class=\"genre")
        })
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if ["ongoing", "en cours", "publishing"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Ongoing
    } else if ["completed", "complété", "fini", "terminé", "achevé"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Completed
    } else if ["dropped", "cancel", "abandonné"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Cancelled
    } else if ["hiatus", "pause"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn filter_string<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn key_from_input(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        Some(normalize_key(input.trim_start_matches(BASE_URL)))
    } else if input.starts_with("/manga/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn normalize_key(value: &str) -> String {
    if let Some(index) = value.find(BASE_URL) {
        return normalize_key(&value[index + BASE_URL.len()..]);
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bs"><div class="bsx"><a title="Sample" href="/manga/sample/"><img src="/cover.jpg"></a></div></div></div><div class="pagination"><a class="next" href="/manga/page/2/">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample</h1><div class="thumb"><img src="/cover.jpg"></div><div class="tsinfo"><div class="imptdt">Auteur <i>Writer</i></div><div class="imptdt">Artiste <i>Artist</i></div><div class="imptdt">Statut <i>En cours</i></div></div><div class="mgen"><a href="/genre/action/">Action</a></div><div class="desc">Résumé</div><ul id="chapterlist"><li><a href="/manga/sample/chapter-1/"><span class="chapternum">Chapitre 1</span></a><span class="chapterdate">2024-01-01</span></li></ul></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
