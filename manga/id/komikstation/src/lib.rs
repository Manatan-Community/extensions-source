use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KomikStation = KomikStation;
const BASE_URL: &str = "https://komikstation.org";
const SOURCE_NAME: &str = "Komik Station";
const CONTENT_RATING: &str = "safe";
const MANGA_DIR: &str = "manga";
const PROJECT_PAGE: Option<&str> = Some("project-list");
const REVERSE_CHAPTERS: bool = false;

struct KomikStation;

impl MangaSource for KomikStation {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing_page(&fetch_document_or_fixture(
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
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(parse_listing_page(&fetch_document_or_fixture(
            &search_url(page, query, None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{MANGA_DIR}/sample"));
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{MANGA_DIR}/sample"));
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body, &key);
        if REVERSE_CHAPTERS {
            chapters.reverse();
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/{MANGA_DIR}/sample/chapter-1"));
        let body = fetch_document_or_fixture(&absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = if let Some(rest) = value.strip_prefix(BASE_URL) {
        rest
    } else {
        value
    };
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn search_url(page: u64, query: &str, order: Option<&str>, filters: Option<&Value>) -> String {
    let mut path = MANGA_DIR.to_string();
    if filter(filters, "project") == Some("project-filter-on") {
        if let Some(project_page) = PROJECT_PAGE {
            path = project_page.to_string();
        }
    }
    let mut params = vec![
        format!("title={}", url::query_escape(query)),
        format!("page={page}"),
    ];
    for (id, name) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
    ] {
        if let Some(value) = filter(filters, id).filter(|value| !value.is_empty()) {
            params.push(format!("{name}={}", url::query_escape(value)));
        }
    }
    let selected_order = filter(filters, "order")
        .filter(|value| !value.is_empty())
        .or(order);
    if let Some(value) = selected_order {
        params.push(format!("order={}", url::query_escape(value)));
    }
    if let Some(genres) = filters.and_then(|value| value.get("genres")) {
        append_genres(&mut params, genres);
    }
    format!("{BASE_URL}/{path}/?{}", params.join("&"))
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters?.get(id)?.as_str()
}

fn append_genres(params: &mut Vec<String>, genres: &Value) {
    if let Some(array) = genres.as_array() {
        for genre in array.iter().filter_map(Value::as_str) {
            if !genre.trim().is_empty() {
                params.push(format!("genre%5B%5D={}", url::query_escape(genre.trim())));
            }
        }
    } else if let Some(value) = genres.as_str() {
        for genre in value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("genre%5B%5D={}", url::query_escape(genre)));
        }
    }
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: parse_listing(body),
        has_next_page: body.contains("pagination") && body.contains("next")
            || body.contains("hpage") && body.contains("class=\"r\""),
    }
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("bsx")
                || chunk.contains("uta")
                || chunk.contains("imgu")
                || chunk.contains("listupd")
                || chunk.contains("animepost")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains(&format!("/{MANGA_DIR}/")) {
                return None;
            }
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)))
                .or_else(|| html::text_between(chunk, "<h4", "</h4>").map(|v| html::strip_tags(&v)))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| SOURCE_NAME.to_string());
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("id".to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique_catalog_item)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/{MANGA_DIR}/sample"));
    let mut tags = link_values(body, "/genre/");
    tags.extend(link_values(body, "?genre"));
    tags.sort();
    tags.dedup();
    let mut description = html::text_between(body, "class=\"desc", "</div>")
        .or_else(|| html::text_between(body, "itemprop=\"description\"", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    if let Some(alt) = info_text(body, "Alternative")
        .or_else(|| info_text(body, "Alternatif"))
        .filter(|value| !value.is_empty())
    {
        description = Some(match description {
            Some(value) => format!("{value}\n\nAlternative: {alt}"),
            None => format!("Alternative: {alt}"),
        });
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| SOURCE_NAME.to_string())),
        cover: html::attr_after(body, "class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        authors: info_text(body, "Author")
            .or_else(|| info_text(body, "Pengarang"))
            .into_iter()
            .collect(),
        artists: info_text(body, "Artist").into_iter().collect(),
        tags,
        description,
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("id".to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-manga-chapter")
                || chunk.contains("chapter")
                || chunk.contains("eph-num")
                || chunk.contains("chbox")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains(BASE_URL) && !href.starts_with('/') {
                return None;
            }
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "class=\"lch", "</a>"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::<MangaChapter>::new(), |mut chapters, chapter| {
            if !chapters.iter().any(|existing| existing.key == chapter.key) {
                chapters.push(chapter);
            }
            chapters
        });
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea")
                || chunk.contains("wp-manga-chapter-img")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| image_attr(chunk))
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .collect::<Vec<_>>();
    pages.extend(script_pages(body));
    pages
        .into_iter()
        .fold(Vec::<String>::new(), |mut images, image| {
            if !images.contains(&image) {
                images.push(image);
            }
            images
        })
        .into_iter()
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

fn script_pages(body: &str) -> Vec<String> {
    let Some((_, tail)) = body.split_once("\"images\"") else {
        return Vec::new();
    };
    let Some(start) = tail.find('[') else {
        return Vec::new();
    };
    let Some(end) = tail[start..].find(']') else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&tail[start..=start + end]).unwrap_or_default()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-lazy-src")
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "data-cfsrc"))
        .or_else(|| srcset_first(html::attr(input, "srcset")))
        .or_else(|| html::attr(input, "src"))
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .find_map(|candidate| candidate.split_whitespace().next().map(ToString::to_string))
            .filter(|value| !value.is_empty())
    })
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("<tr")
        .chain(body.split("imptdt"))
        .chain(body.split("fmed"))
        .find_map(|chunk| {
            if !chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
            {
                return None;
            }
            html::text_between(chunk, "<td", "</td>")
                .or_else(|| html::text_between(chunk, "<i", "</i>"))
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
                .map(|value| html::strip_tags(&value).replace(label, ""))
                .map(|value| value.trim_matches(':').trim().to_string())
                .filter(|value| !value.is_empty() && value != "-" && value != "N/A")
        })
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("hiatus") || lower.contains("on hold") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") || lower.contains("cancelled") || lower.contains("canceled")
    {
        ItemStatus::Cancelled
    } else if lower.contains("completed")
        || lower.contains("complete")
        || lower.contains("tamat")
        || lower.contains("finished")
    {
        ItemStatus::Completed
    } else if lower.contains("ongoing") || lower.contains("on going") || lower.contains("berjalan")
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique_catalog_item(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd"><div class="bsx"><a href="/manga/sample" title="Sample Manga"><img data-src="/cover.jpg"></a></div></div>
<div class="pagination"><a class="next page-numbers" href="/manga/?page=2">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div><div class="desc">Sample description.</div><div class="tsinfo"><div class="imptdt">Status <i>Ongoing</i></div></div><div class="mgen"><a href="/genre/action">Action</a></div></div>
<div id="chapterlist"><ul><li><div class="eph-num"><a href="/manga/sample/chapter-1"><span class="chapternum">Chapter 1</span></a></div><span class="chapterdate">January 1, 2024</span></li></ul></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img data-src="/page2.jpg"></div>"#;
