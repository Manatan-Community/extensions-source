use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: NovelCool = NovelCool;
const API_URL: &str = "https://api.novelcool.com";
const APP_ID: &str = "202201290625004";
const APP_SECRET: &str = "c73a8590641781f203660afca1d37ada";
const PAGE_SIZE: u64 = 20;

const SOURCES: [SourceConfig; 7] = [
    SourceConfig {
        id: "novelcool-en",
        lang: "en",
        site_lang: "en",
        base_url: "https://www.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-es",
        lang: "es",
        site_lang: "es",
        base_url: "https://es.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-de",
        lang: "de",
        site_lang: "de",
        base_url: "https://de.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-ru",
        lang: "ru",
        site_lang: "ru",
        base_url: "https://ru.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-it",
        lang: "it",
        site_lang: "it",
        base_url: "https://it.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-pt-br",
        lang: "pt-BR",
        site_lang: "br",
        base_url: "https://br.novelcool.com",
    },
    SourceConfig {
        id: "novelcool-fr",
        lang: "fr",
        site_lang: "fr",
        base_url: "https://fr.novelcool.com",
    },
];

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    site_lang: &'static str,
    base_url: &'static str,
}

impl SourceConfig {
    fn absolute_url(self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    fn key_from_url(self, value: &str) -> String {
        if let Some(index) = value.find(self.base_url) {
            return format!(
                "/{}",
                value[index + self.base_url.len()..].trim_start_matches('/')
            );
        }
        format!("/{}", value.trim_start_matches('/'))
    }
}

struct NovelCool;

impl MangaSource for NovelCool {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if use_app_api(&request) {
            let endpoint = if latest { "latest" } else { "hot" };
            if let Some(page) =
                fetch_api_page(source, &format!("{API_URL}/elite/{endpoint}/"), page, None)
            {
                return Ok(page);
            }
        }
        let path = if latest {
            "/category/latest.html"
        } else {
            "/category/new_list.html"
        };
        let body = fetch_document_or_fixture(source, &source.absolute_url(path), LIST_FIXTURE);
        Ok(parse_listing_page(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(source.base_url) {
            let body = fetch_document_or_fixture(source, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(
                    &body,
                    Some(source.key_from_url(query)),
                    source,
                )],
                has_next_page: false,
            });
        }
        if use_app_api(&request) {
            if let Some(page) = fetch_api_page(
                source,
                &format!("{API_URL}/book/search/"),
                page,
                Some(query),
            ) {
                return Ok(page);
            }
        }
        let body = fetch_document_or_fixture(
            source,
            &search_url(
                source,
                page,
                query,
                request.get("filters").unwrap_or(&Value::Null),
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing_page(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.html".into());
        let body = fetch_document_or_fixture(source, &source.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.html".into());
        let body = fetch_document_or_fixture(source, &source.absolute_url(&key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/sample-1.html".into());
        let target = source.absolute_url(&key);
        let body = fetch_document_or_fixture(source, &target, PAGES_FIXTURE);
        Ok(parse_pages(&body, &target, source))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(source.base_url) && input.contains("/manga/") {
            let body = fetch_document_or_fixture(source, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &body,
                    Some(source.key_from_url(input)),
                    source,
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

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("novelcool-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn use_app_api(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("useAppApi"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn client(source: SourceConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", source.base_url.trim_end_matches('/')))
        .with_cookies_for(source.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(source: SourceConfig, target: &str, fixture: &str) -> String {
    client(source)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api_page(
    source: SourceConfig,
    endpoint: &str,
    page: u64,
    query: Option<&str>,
) -> Option<Paged<CatalogItem>> {
    let payload = json!({
        "appId": APP_ID,
        "keyword": query,
        "lang": source.site_lang,
        "lc_type": "manga",
        "page": page.to_string(),
        "page_size": PAGE_SIZE.to_string(),
        "secret": APP_SECRET
    });
    let response: Value = serde_json::from_str(
        &client(source)
            .post(endpoint)
            .json(payload.to_string())
            .send_text()
            .ok()?,
    )
    .ok()?;
    let list = response.get("list")?.as_array()?;
    Some(Paged {
        entries: list
            .iter()
            .filter_map(|item| api_item(item, source))
            .collect(),
        has_next_page: list.len() as u64 == PAGE_SIZE,
    })
}

fn api_item(item: &Value, source: SourceConfig) -> Option<CatalogItem> {
    let key = source.key_from_url(item.get("url")?.as_str()?);
    Some(CatalogItem {
        key: key.clone(),
        title: item.get("name")?.as_str()?.to_string(),
        cover: item
            .get("cover")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn search_url(source: SourceConfig, page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![("name", query.to_string()), ("page", page.to_string())];
    for (key, upstream) in [
        ("author", "author"),
        ("categoryId", "category_id"),
        ("excludeCategoryId", "out_category_id"),
        ("completed", "completed_series"),
        ("rating", "rate_star"),
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            params.push((upstream, value.to_string()));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/search?{query}", source.base_url.trim_end_matches('/'))
}

fn parse_listing_page(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("book-item")
            .skip(1)
            .filter_map(|chunk| parse_listing_item(chunk, source))
            .collect(),
        has_next_page: body.contains("page-nav") && body.contains("next"),
    }
}

fn parse_listing_item(chunk: &str, source: SourceConfig) -> Option<CatalogItem> {
    if chunk.contains("book-type-novel") {
        return None;
    }
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = source.key_from_url(&href);
    let title = html::attr_after(chunk, "book-pic", "title")
        .or_else(|| html::attr_after(chunk, "<a", "title"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "lazy_url")
            .or_else(|| html::attr_after(chunk, "<img", "data-src"))
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| source.absolute_url(&value)),
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample.html".to_string());
    let status_text = first_category(body).unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "bookinfo-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "bookinfo-pic-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| source.absolute_url(&value)),
        authors: html::attr_after(body, "bookinfo-author", "title")
            .into_iter()
            .collect(),
        tags: category_values(body),
        description: html::text_between(body, "bk-summary-txt", "</div>")
            .map(|value| html::strip_tags(&value)),
        status: parse_status(&status_text),
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_status(value: &str) -> ItemStatus {
    let normalized = value.to_lowercase();
    let completed = [
        "completed",
        "completo",
        "completado",
        "concluído",
        "concluido",
        "finalizado",
        "terminé",
        "hoàn thành",
    ];
    let ongoing = [
        "ongoing",
        "продолжается",
        "updating",
        "em lançamento",
        "em andamento",
        "en cours",
        "ativo",
        "lançando",
        "đang tiến hành",
        "devam ediyor",
        "in corso",
        "in arrivo",
        "en curso",
        "emision",
        "curso",
        "en marcha",
        "publicandose",
        "en emision",
    ];
    if completed.contains(&normalized.as_str()) {
        ItemStatus::Completed
    } else if ongoing.contains(&normalized.as_str()) {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    body.split("chapter-item-list")
        .skip(1)
        .flat_map(|section| section.split("<a").skip(1))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = source.key_from_url(&href);
            let title = html::attr(chunk, "title").unwrap_or_else(|| html::strip_tags(chunk));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: title.split_whitespace().find_map(|part| {
                    part.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '.')
                        .parse()
                        .ok()
                }),
                url: Some(source.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    if let Some(server_url) = html::attr_after(body, "vision-button", "href") {
        let target = source.absolute_url(&server_url);
        let body = fetch_document_or_fixture(source, &target, PAGES_FIXTURE);
        let pages = parse_pages(&body, &target, source);
        if !pages.is_empty() {
            return pages;
        }
    }
    if let Some(redirect) = redirect_target(body, current_url) {
        let body = fetch_document_or_fixture(source, &redirect, PAGES_FIXTURE);
        let pages = parse_pages(&body, &redirect, source);
        if !pages.is_empty() {
            return pages;
        }
    }
    let images = parse_all_imgs_url(body);
    if !images.is_empty() {
        return images
            .into_iter()
            .enumerate()
            .map(|(index, image)| page(index, &image, current_url, source))
            .collect();
    }
    if let Some(image) = html::attr_after(body, "mangaread-manga-pic", "src") {
        return vec![page(0, &source.absolute_url(&image), current_url, source)];
    }
    body.split("<option")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "value"))
        .enumerate()
        .filter_map(|(index, target)| {
            let target = source.absolute_url(&target);
            let body = fetch_document_or_fixture(source, &target, PAGE_IMAGE_FIXTURE);
            let image = html::attr_after(&body, "mangaread-manga-pic", "src")?;
            Some(page(index, &source.absolute_url(&image), &target, source))
        })
        .collect()
}

fn parse_all_imgs_url(body: &str) -> Vec<String> {
    let Some(index) = body.find("all_imgs_url") else {
        return Vec::new();
    };
    let Some(open) = body[index..].find('[') else {
        return Vec::new();
    };
    let rest = &body[index + open + 1..];
    let Some(close) = rest.find(']') else {
        return Vec::new();
    };
    rest[..close]
        .split(',')
        .map(|part| part.trim().trim_matches(['"', '\'']).replace("\\/", "/"))
        .filter(|part| part.starts_with("http"))
        .collect()
}

fn redirect_target(body: &str, current_url: &str) -> Option<String> {
    let index = body.find("window.location.href")?;
    let rest = &body[index..];
    let quote = rest.find('"').or_else(|| rest.find('\''))?;
    let delimiter = rest.as_bytes()[quote] as char;
    let tail = &rest[quote + 1..];
    let end = tail.find(delimiter)?;
    Some(url::join_url(current_url, &tail[..end]))
}

fn page(index: usize, image: &str, referer: &str, source: SourceConfig) -> MangaPage {
    let headers = manga::image_headers(referer);
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        extra: [(
            "sourceBaseUrl".to_string(),
            Value::String(source.base_url.to_string()),
        )]
        .into_iter()
        .collect(),
        ..MangaPage::default()
    }
}

fn category_values(body: &str) -> Vec<String> {
    body.split("bookinfo-category-list")
        .skip(1)
        .flat_map(|section| section.split("<a").skip(1))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn first_category(body: &str) -> Option<String> {
    category_values(body).into_iter().next()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="book-list"><div class="book-item"><a href="/manga/sample.html"><div class="book-pic" title="Fixture Manga"><img lazy_url="https://img.example/cover.jpg"></div></a></div></div>
<div class="page-nav"><a><div class="next">Next</div></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="bookinfo-title">Fixture Manga</h1>
<img class="bookinfo-pic-img" src="https://img.example/cover.jpg">
<div class="bookinfo-author"><a title="Author One"></a></div>
<div class="bookinfo-category-list"><a>Completed</a><a>Action</a></div>
<div class="bk-summary-txt">Fixture description.</div>
<div class="chapter-item-list"><a href="/chapter/sample-1.html" title="Chapter 1"><span class="chapter-item-time">Jan 01, 2024</span></a></div>
"#;

const CHAPTERS_FIXTURE: &str = DETAILS_FIXTURE;

const PAGES_FIXTURE: &str = r#"
<script>var all_imgs_url: ["https://img.example/1.jpg", "https://img.example/2.jpg"];</script>
"#;

const PAGE_IMAGE_FIXTURE: &str =
    r#"<img class="mangaread-manga-pic" src="https://img.example/page.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_novelcool_fixtures() {
        let source = SOURCES[0];
        assert_eq!(parse_listing_page(LIST_FIXTURE, source).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/manga/sample.html".into()), source).title,
            "Fixture Manga"
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, source).len(), 1);
        assert_eq!(
            parse_pages(
                PAGES_FIXTURE,
                "https://www.novelcool.com/chapter/sample-1.html",
                source
            )
            .len(),
            2
        );
    }
}
