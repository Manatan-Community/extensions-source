use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: TrManga = TrManga;
const BASE_URL: &str = "https://trmanga.com";

struct TrManga;

impl MangaSource for TrManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let target = if listing == "latest" {
            format!("{BASE_URL}/son-eklenenler?page={page}")
        } else {
            format!("{BASE_URL}/webtoon-listesi?sort=views&short_type=DESC&page={page}")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        if listing == "latest" {
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
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = format!(
            "{BASE_URL}/webtoon-listesi?page={page}&q={}&genre={}&sort={}&short_type={}&status={}",
            url::query_escape(query),
            url::query_escape(filter_str(filters, "genre").unwrap_or("")),
            url::query_escape(filter_str(filters, "sort").unwrap_or("views")),
            url::query_escape(filter_str(filters, "order").unwrap_or("DESC")),
            url::query_escape(filter_str(filters, "status").unwrap_or(""))
        );
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_popular(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/webtoon/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/webtoon/sample/1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("col-xl-4")
        .skip(1)
        .filter_map(|chunk| catalog_from_chunk(chunk, "a[class]"))
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-link") && body.contains("Sonraki"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("span") && chunk.contains("title") && chunk.contains("<img"))
        .filter_map(|chunk| catalog_from_chunk(chunk, "<a"))
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("page-link") && body.contains("Sonraki"),
    }
}

fn catalog_from_chunk(chunk: &str, marker: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, marker, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "span class=\"title\"", "</")
        .or_else(|| html::text_between(chunk, "<a", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("tr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/webtoon/sample".to_string());
    let author = info_after(body, "Yazar &amp; Çizer İsim(ler) :")
        .or_else(|| info_after(body, "Yazar & Çizer İsim(ler) :"));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "movie__title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "movie__plot", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        status: parse_status(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("tr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Chapter".to_string()));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: first_number(&title),
                scanlators: html::text_between(chunk, "<td", "</td>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .into_iter()
                    .collect(),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let text = info_after(body, "Durum :").unwrap_or_default().to_lowercase();
    if ["ongoing", "devam ediyor", "guncel", "güncel"]
        .iter()
        .any(|needle| text.contains(needle))
    {
        ItemStatus::Ongoing
    } else if ["complete", "tamamlandi", "tamamlandı", "bitti"]
        .iter()
        .any(|needle| text.contains(needle))
    {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn info_after(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn first_number(input: &str) -> Option<f32> {
    let mut number = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
        } else if !number.is_empty() {
            break;
        }
    }
    number.parse().ok()
}

fn filter_str<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters.get(id).and_then(Value::as_str)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(BASE_URL) {
        return format!(
            "/{}",
            value[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="col-xl-4"><a class="title" href="/webtoon/sample">Sample</a><img data-src="/cover.jpg"></div><a class="page-link">Sonraki</a>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="movie__title">Sample</h1><meta property="og:image" content="/cover.jpg"><p>Yazar &amp; Çizer İsim(ler) : Author</p><p>Durum :<span>devam ediyor</span></p><div class="movie__plot">Desc</div><table><tbody><tr><td><a href="/webtoon/sample/1">1. Bolum</a></td><td><a>Team</a></td></tr></tbody></table>"#;
const PAGES_FIXTURE: &str = r#"<img data-src="/page1.jpg">"#;
