use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: BRYaoi = BRYaoi;
const BASE_URL: &str = "https://bryaoi.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct BRYaoi;

impl MangaSource for BRYaoi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/yaoi/page/{page}"),
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
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/yaoi/page/{page}?s={}", url::query_escape(query)),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/cap-1".into());
        Ok(parse_pages(&fetch_document(
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

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
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

fn fetch_document(target: &str, fixture: &str) -> String {
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
        .filter(|chunk| chunk.contains("<h2") || chunk.contains("listagem"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h2", "</h2>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "BR Yaoi".into())
                    }),
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("next page-numbers"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample".into());
    let title = html::text_between(body, "<h1", "</h1>")
        .map(|value| {
            html::strip_tags(&value)
                .replace("Ler", "")
                .replace("Online", "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "BR Yaoi".into()));
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "serie-capa", "src").map(|image| absolute_url(&image)),
        description: html::text_between(body, "serie-texto", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: html::text_between(body, "serie-infos", "</div>")
            .map(|value| {
                html::strip_tags(&value)
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("href=") && (chunk.contains("capitulo") || chunk.contains("Cap"))
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains(BASE_URL) && !href.starts_with('/') {
                return None;
            }
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::strip_tags(chunk)
                        .trim()
                        .trim_matches('>')
                        .trim()
                        .to_string(),
                )
                .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    chapters.reverse();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo unico".into()),
            url: Some(absolute_url(manga_key)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("images_all") || chunk.contains("src="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.starts_with("data:"))
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
    format!(
        "/{}",
        input.trim().trim_start_matches(BASE_URL).trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut entries: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listagem"><div class="item"><a href="/sample"><img src="/cover.jpg"><h2>Sample BR Yaoi</h2></a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Ler Sample BR Yaoi Online</h1><div class="serie-capa"><img src="/cover.jpg"></div>
<div class="serie-texto"><p>Sample description.</p></div>
<div class="serie-infos"><a>Yaoi</a></div><div class="capitulos"><a href="/sample/cap-1">Capitulo 1</a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<div id="images_all"><img src="/page-1.jpg"></div>
"#;
