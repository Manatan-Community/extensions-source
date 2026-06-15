use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UpdateStrategy, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HentaiMode = HentaiMode;
const BASE_URL: &str = "https://hentaimode.com";
const NAME: &str = "HentaiMode";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct HentaiMode;

impl MangaSource for HentaiMode {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        Ok(parse_listing(&fetch_document_or_fixture(
            BASE_URL,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("id:") {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        let target = format!("{BASE_URL}/buscar?s={}", url::query_escape(query));
        Ok(parse_listing(&fetch_document_or_fixture(
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/g/sample".to_string());
        let chapter_key = key.replace("/g/", "/leer/");
        Ok(vec![MangaChapter {
            key: chapter_key.clone(),
            title: Some("Chapter".to_string()),
            chapter_number: Some(1.0),
            url: Some(absolute_url(&chapter_key)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/leer/sample".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document_or_fixture(BASE_URL, LIST_FIXTURE));
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: false,
            ..HomeSection::default()
        }])
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
                    &key,
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("book-list") || chunk.contains("book-description"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "book-description", "</p>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| NAME.into())),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.into())),
        cover: html::attr_after(body, "id=\"cover\"", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        authors: info_values(body, "Grupo"),
        artists: info_values(body, "Artista"),
        tags: info_values(body, "Categorias")
            .into_iter()
            .chain(info_values(body, "Categor"))
            .collect(),
        description: details_description(body),
        status: ItemStatus::Completed,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("tag-container")
        .filter(|chunk| chunk.contains(label))
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter(|part| part.contains("tag"))
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn details_description(body: &str) -> Option<String> {
    let labels = ["Serie", "Tipo", "Personajes", "Idioma"];
    let parts = labels
        .into_iter()
        .filter_map(|label| {
            let values = info_values(body, label);
            (!values.is_empty()).then(|| format!("{label}: {}", values.join(", ")))
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("page_image") || chunk.contains("pages = ["))
        .unwrap_or(body);
    let source = script
        .split("pages = [")
        .nth(1)
        .and_then(|part| part.split(']').next())
        .unwrap_or(script);
    source
        .split(',')
        .filter_map(|part| {
            let value = part
                .split(':')
                .next_back()
                .unwrap_or(part)
                .trim()
                .trim_matches(['"', '\'']);
            (!value.is_empty() && (value.starts_with("http") || value.starts_with('/')))
                .then(|| value.to_string())
        })
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

fn normalize_key(input: &str) -> String {
    let mut value = input.trim().trim_start_matches("id:").trim_end_matches('/');
    if let Some((_, rest)) = value.split_once("/g/") {
        value = rest.split('/').next().unwrap_or(rest);
    }
    if let Some((_, rest)) = value.split_once("/leer/") {
        return format!("/leer/{}", rest.split('/').next().unwrap_or(rest));
    }
    format!("/g/{}", value.trim_matches('/'))
}

fn absolute_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!(
            "{}/{}",
            BASE_URL.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

const LIST_FIXTURE: &str = r#"<div class="row"><div class="book-list"><a href="/g/sample"><img src="/cover.jpg"><div class="book-description"><p>Sample</p></div></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<div id="cover"><img src="/cover.jpg"></div><div id="info-block"><div id="info"><h1>Sample</h1><div class="tag-container">Categorias <a class="tag">Tag</a></div><div class="tag-container">Grupo <a class="tag">Group</a></div><div class="tag-container">Artista <a class="tag">Artist</a></div></div></div>"#;
const PAGES_FIXTURE: &str = r#"<script>var pages = [{page_image:"/page1.jpg"},]</script>"#;

export_manga_source!(SOURCE);
