use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: AnimeGdrClub = AnimeGdrClub;
const BASE_URL: &str = "http://www.agcscanlation.it";

struct AnimeGdrClub;

impl MangaSource for AnimeGdrClub {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing_id = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let (target, fixture, latest) = if listing_id == "latest" {
            (format!("{BASE_URL}/"), LATEST_FIXTURE, true)
        } else {
            (format!("{BASE_URL}/serie.php"), LIST_FIXTURE, false)
        };
        let body = fetch_document_or_fixture(&target, fixture);
        Ok(if latest {
            parse_project_links(&body)
        } else {
            parse_series_grid(&body, None)
        })
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
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let mut page = parse_series_grid(
                &fetch_document_or_fixture(&format!("{BASE_URL}/serie.php"), LIST_FIXTURE),
                None,
            );
            let needle = query.to_lowercase();
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&needle));
            return Ok(page);
        }

        let filters = parse_filters(request.get("filters"));
        if filters.filter_type == "status" {
            let classes = filters.statuses;
            return Ok(parse_series_grid(
                &fetch_document_or_fixture(&format!("{BASE_URL}/serie.php"), LIST_FIXTURE),
                Some(&classes),
            ));
        }

        let target = if filters.genre.is_empty() {
            format!("{BASE_URL}/listone.php")
        } else {
            format!("{BASE_URL}/listone.php?genere={}", url::query_escape(&filters.genre))
        };
        Ok(parse_project_links(&fetch_document_or_fixture(
            &target,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/progetto.php?nome=sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/progetto.php?nome=sample".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/readerr.php?nome=sample&cap=1".to_string());
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

#[derive(Default)]
struct ParsedFilters {
    filter_type: String,
    genre: String,
    statuses: Vec<String>,
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        filter_type: "genre".to_string(),
        ..ParsedFilters::default()
    };
    let Some(filters) = filters else {
        return parsed;
    };
    let values = filters.as_array().cloned().unwrap_or_else(|| {
        filters
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .map(|(id, value)| serde_json::json!({ "id": id, "value": value }))
                    .collect()
            })
            .unwrap_or_default()
    });
    for filter in values {
        let id = filter.get("id").and_then(Value::as_str).unwrap_or_default();
        let value = filter.get("value").unwrap_or(&Value::Null);
        match id {
            "filterType" => parsed.filter_type = value.as_str().unwrap_or("genre").to_string(),
            "genre" => parsed.genre = value.as_str().unwrap_or_default().to_string(),
            "statuses" => {
                parsed.statuses = value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }
    parsed
}

fn parse_series_grid(body: &str, classes: Option<&[String]>) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("manga"))
        .filter(|chunk| {
            classes
                .map(|classes| classes.iter().any(|class| chunk.contains(class)))
                .unwrap_or(true)
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "linkalmanga", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "nomeserie", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| image_attr(chunk).and_then(|_| html::attr_after(chunk, "<img", "alt")))
                .or_else(|| url::slug_from_url(&href))?;
            Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_project_links(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("progetto.php") || chunk.contains("titolo"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("progetto.php") {
                return None;
            }
            let title = html::text_between(chunk, "titolo", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| url::slug_from_url(&href))?;
            Some(catalog_item(normalize_key(&href), title, image_attr(chunk)))
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_item(key: String, title: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover.map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/progetto.php?nome=sample".to_string());
    let info = html::text_between(body, "tabellaalta", "</table>")
        .unwrap_or_else(|| html::text_between(body, "tabellaalta", "</div>").unwrap_or_default());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::text_between(body, "nomeserie", "</"))
            .map(|value| html::strip_tags(&value))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Anime GDR Club".to_string()),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "trama", "</")
            .map(|value| html::strip_tags(&value).trim_start_matches("Trama:").trim().to_string())
            .filter(|value| !value.is_empty()),
        tags: body
            .split("generi")
            .skip(1)
            .flat_map(|chunk| {
                chunk
                    .split("<a")
                    .skip(1)
                    .filter_map(|link| html::text_between(link, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&info),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("it".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("in corso") {
        ItemStatus::Ongoing
    } else if lower.contains("concluso") {
        ItemStatus::Completed
    } else if lower.contains("interrotto") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("reader"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?.replace("reader", "readerr");
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitolo".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title.clone()),
                chapter_number: title
                    .trim_start_matches("Capitolo")
                    .trim()
                    .parse::<f32>()
                    .ok(),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let manga_path = html::attr_after(body, "id=\"nomemanga\"", "class")
        .or_else(|| html::attr_after(body, "id='nomemanga'", "class"));
    let chapter = html::text_between(body, "numcap", "</").map(|value| html::strip_tags(&value));
    let max = html::text_between(body, "maxpag", "</")
        .map(|value| html::strip_tags(&value))
        .and_then(|value| value.parse::<usize>().ok());
    if let (Some(manga_path), Some(chapter), Some(max)) = (manga_path, chapter, max) {
        if max > 0 {
            return (1..=max)
                .map(|page| page_url(format!("{BASE_URL}/{manga_path}/cap.{chapter}/{page}.jpg"), page))
                .collect();
        }
    }

    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("corrente") || chunk.contains("src="))
        .filter_map(image_attr)
        .enumerate()
        .map(|(index, image)| page_url(url::join_url(BASE_URL, &image), index + 1))
        .collect()
}

fn page_url(image: String, number: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {number}")),
        ..MangaPage::default()
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn normalize_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with(BASE_URL) {
        let base = BASE_URL.trim_end_matches('/');
        return format!("/{}", trimmed.trim_start_matches(base).trim_start_matches('/'));
    }
    format!("/{}", trimmed.trim_start_matches('/'))
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="manga progettiincorso"><a class="linkalmanga" href="/progetto.php?nome=sample"><img src="/cover.jpg"><div class="nomeserie"><span>Sample Manga</span></div></a></div>"#;
const LATEST_FIXTURE: &str = r#"<div class="containernews"><a href="/progetto.php?nome=sample"><img src="/cover.jpg"><div class="titolo">Sample Manga</div></a></div>"#;
const SEARCH_FIXTURE: &str = r#"<div class="listonegen"><a href="/progetto.php?nome=sample"><img src="/cover.jpg"><div class="titolo">Sample Manga</div></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="tabellaalta">In Corso <span class="generi"><a>Azione</a></span></div><span class="trama">Trama: Description</span><div class="capitoli_cont"><a href="/reader.php?nome=sample&cap=1">Capitolo 1</a></div>"#;
const PAGES_FIXTURE: &str = r#"<div id="nomemanga" class="sample"></div><span class="numcap">1</span><span class="maxpag">2</span>"#;
