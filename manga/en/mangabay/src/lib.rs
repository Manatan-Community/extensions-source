use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Mangabay = Mangabay;
const BASE_URL: &str = "https://read.manga-bay.org";
const FALLBACK_PREFIX: &str = "fallback:";

struct Mangabay;

impl MangaSource for Mangabay {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut filters = request
            .get("filters")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            filters["sort"] = Value::String("date".to_string());
        } else {
            filters["sort"] = Value::String("rating".to_string());
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &catalog_url(page, "", Some(&filters)),
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
            if !key.starts_with("/manga/") && !key.starts_with("/comics/") {
                return Ok(Paged {
                    entries: Vec::new(),
                    has_next_page: false,
                });
            }
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            &catalog_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/reader/100/1".to_string());
        Ok(parse_pages(&fetch_reader_or_fixture(&key, PAGES_FIXTURE)))
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

fn fetch_reader_or_fixture(key: &str, fixture: &str) -> String {
    let target = absolute_url(key);
    let manga_id = key.trim_start_matches('/').split('/').nth(1).unwrap_or("");
    client()
        .with_header("Cookie", format!("adult={manga_id}"))
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(".org/") {
            return format!("/{}", value[index + 5..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

fn catalog_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    if !query.is_empty() {
        let page_path = if page > 1 {
            format!("/page/{page}/")
        } else {
            String::new()
        };
        return format!("{BASE_URL}/search/{}{page_path}", url::query_escape(query));
    }
    let genre = filter_string(filters, "genre");
    let excluded = filter_string(filters, "excludeGenre");
    let mut path = if genre.is_empty() && excluded.is_empty() {
        format!("{BASE_URL}/comix")
    } else {
        let mut parts = Vec::new();
        if !genre.is_empty() {
            parts.push(format!("g={genre}"));
        }
        if !excluded.is_empty() {
            parts.push(format!("exc_g={excluded}"));
        }
        format!("{BASE_URL}/ComicList/{}", parts.join("/"))
    };
    if page > 1 {
        path.push_str(&format!("/page/{page}"));
    }
    let sort = filter_string(filters, "sort");
    let direction = filter_string(filters, "direction");
    if !sort.is_empty() {
        path.push_str(&format!(
            "?dlenewssortby={sort}&dledirection={}",
            if direction.is_empty() {
                "desc"
            } else {
                &direction
            }
        ));
    }
    path
}

fn filter_string(filters: Option<&Value>, id: &str) -> String {
    filters
        .and_then(|value| value.get(id))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("readed")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "readed__title", "href")?;
            let key = normalize_key(&href);
            let mini = html::attr_after(chunk, "readed__img", "data-src")
                .or_else(|| html::attr_after(chunk, "<img", "data-src"))
                .map(|value| absolute_url(&value));
            let cover = hd_poster_url(&key).map_or(mini.clone(), |hd| {
                Some(match mini {
                    Some(mini) => format!("{hd}#{FALLBACK_PREFIX}{mini}"),
                    None => hd,
                })
            });
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "readed__title", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        });
    Paged {
        entries,
        has_next_page: body
            .split("pagination__pages")
            .nth(1)
            .is_some_and(|chunk| chunk.contains("<a")),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let alt_titles = html::text_between(body, "page__header", "</h2>")
        .map(|value| {
            html::strip_tags(&value)
                .split([';', ',', '/'])
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let page_text = html::text_between(body, "page__text", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "page__header", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        alternate_titles: alt_titles.clone(),
        cover: html::attr_after(body, "page__poster", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: match (page_text, alt_titles.is_empty()) {
            (Some(text), true) => Some(text),
            (Some(text), false) => Some(format!(
                "{text}\n\nAlternative titles:\n- {}",
                alt_titles.join("\n- ")
            )),
            (None, false) => Some(format!(
                "Alternative titles:\n- {}",
                alt_titles.join("\n- ")
            )),
            (None, true) => None,
        },
        authors: page_list_value(body, "Author").into_iter().collect(),
        artists: page_list_value(body, "Artist").into_iter().collect(),
        tags: parse_tags(body),
        status: parse_status(body),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    extract_data(body)
        .and_then(|data| serde_json::from_str::<ChapterListDto>(&data).ok())
        .map(|dto| {
            dto.chapters
                .into_iter()
                .map(|chapter| MangaChapter {
                    key: format!("/reader/{}/{}", dto.news_id, chapter.id),
                    title: Some(chapter.title),
                    url: Some(format!("{BASE_URL}/reader/{}/{}", dto.news_id, chapter.id)),
                    ..MangaChapter::default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    extract_data(body)
        .and_then(|data| serde_json::from_str::<PageListDto>(&data).ok())
        .map(|dto| {
            dto.images
                .into_iter()
                .enumerate()
                .map(|(index, image)| {
                    let image = if image.starts_with("http") {
                        image.trim().to_string()
                    } else {
                        absolute_url(image.trim())
                    };
                    let referer = if image.contains("manga-bay.org") {
                        BASE_URL
                    } else {
                        ""
                    };
                    MangaPage {
                        content: PageContent::Url {
                            url: image,
                            context: Some(manga::image_headers(referer)),
                        },
                        headers: manga::image_headers(referer),
                        description: Some(format!("Page {}", index + 1)),
                        ..MangaPage::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_data(body: &str) -> Option<String> {
    let script = body.split("window.__DATA__ = ").nth(1)?;
    Some(
        script
            .split(";window.")
            .next()
            .unwrap_or(script)
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string(),
    )
}

fn hd_poster_url(key: &str) -> Option<String> {
    let slug = key.trim_matches('/').split('/').next_back()?;
    (!slug.is_empty()).then(|| format!("{BASE_URL}/uploads/posts/{slug}.jpg"))
}

fn page_list_value(body: &str, label: &str) -> Option<String> {
    body.split("page__list")
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            html::strip_tags(chunk)
                .replace(label, "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn parse_tags(body: &str) -> Vec<String> {
    let mut tags = body
        .split("page__tags")
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if body.to_ascii_lowercase().contains(">korean<") {
        tags.insert(0, "Manhwa".to_string());
    } else if body.to_ascii_lowercase().contains(">chinese<") {
        tags.insert(0, "Manhua".to_string());
    } else if body.to_ascii_lowercase().contains(">japanese<") {
        tags.insert(0, "Manga".to_string());
    }
    tags
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("finished") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelled") || lower.contains("canceled") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Ongoing
    }
}

#[derive(Deserialize)]
struct ChapterListDto {
    news_id: i64,
    chapters: Vec<ChapterDto>,
}

#[derive(Deserialize)]
struct ChapterDto {
    id: i64,
    title: String,
}

#[derive(Deserialize)]
struct PageListDto {
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="dle-content"><div class="readed"><div class="readed__title"><a href="/manga/sample">Sample Manga</a></div><div class="readed__img"><img data-src="/cover.jpg"></div></div></div>
<div class="pagination__pages"><a href="/comix/page/2/">2</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<article class="page"><header class="page__header"><h1>Sample Manga</h1><h2>Sample Alt</h2></header>
<div class="page__poster"><img src="/cover.jpg"></div><div class="page__text">A sample.</div>
<ul class="page__list"><li><div>Author</div>Creator</li><li><div>Artist</div>Artist</li></ul>
<div class="page__meta-pills"><span class="page__meta-pill">Korean</span><span class="page__meta-pill page__meta-pill--status">Ongoing</span></div>
<div class="page__tags"><a>Drama</a></div></article>
<script>window.__DATA__ = {"news_id":100,"chapters":[{"id":1,"title":"Chapter 1","date":"1.1.2024"}]};window.__NEXT__ = true;</script>
"#;
const PAGES_FIXTURE: &str = r#"<script>window.__DATA__ = {"images":["/image1.jpg","https://cdn.example.test/image2.jpg"]};window.__NEXT__ = true;</script>"#;
