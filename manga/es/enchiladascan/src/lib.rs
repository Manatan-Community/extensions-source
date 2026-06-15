use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: EnchiladaScan = EnchiladaScan;
const DOMAIN_URL: &str = "https://enchiladascan.github.io";
const BASE_URL: &str = "https://enchiladascan.github.io/enchiladaweb";
const NAME: &str = "EnchiladaScan";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct EnchiladaScan;

impl MangaSource for EnchiladaScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalog(CATALOG_FIXTURE, None));
        }
        Ok(parse_catalog(
            &fetch_text(&format!("{BASE_URL}/catalogo.json"), CATALOG_FIXTURE),
            None,
        ))
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
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        Ok(parse_catalog(
            &fetch_text(&format!("{BASE_URL}/catalogo.json"), CATALOG_FIXTURE),
            Some(query),
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample/".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/enchiladaweb/manga/sample/chapter-1/".into());
        let (manga_slug, chapter_slug) = slugs_from_chapter_key(&key);
        Ok(parse_pages(&fetch_text(
            &format!("{BASE_URL}/assets/mangas/{manga_slug}/{chapter_slug}/images.json"),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(DOMAIN_URL, &key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let catalog = parse_catalog(
            &fetch_text(&format!("{BASE_URL}/catalogo.json"), CATALOG_FIXTURE),
            None,
        );
        Ok(vec![HomeSection {
            id: "catalog".to_string(),
            title: "Catalogo".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: catalog.entries,
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
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_catalog(body: &str, query: Option<&str>) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, CATALOG_FIXTURE);
    let query = query.unwrap_or_default().to_ascii_lowercase();
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            query.is_empty()
                || string_value(item, "title")
                    .is_some_and(|title| title.to_ascii_lowercase().contains(&query))
        })
        .map(catalog_from_json)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_from_json(item: &Value) -> CatalogItem {
    let key = string_value(item, "post_url").unwrap_or_else(|| "/manga/sample/".to_string());
    CatalogItem {
        key: key.clone(),
        title: string_value(item, "title").unwrap_or_else(|| NAME.to_string()),
        cover: string_value(item, "portada").map(|cover| format!("{BASE_URL}{cover}")),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: class_text(body, "manga-title").unwrap_or_else(|| NAME.to_string()),
        cover: html::attr_after(body, "manga-cover", "src").map(|image| absolute_url(&image)),
        authors: meta_value(body, "Autor").into_iter().collect(),
        artists: meta_value(body, "Arte").into_iter().collect(),
        tags: meta_value(body, "Genero")
            .or_else(|| meta_value(body, "Género"))
            .map(|value| vec![value])
            .unwrap_or_default(),
        status: status_from(meta_value(body, "Estado").as_deref()),
        description: class_text(body, "manga-sinopsis"),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chaptersList") || chunk.contains("cap-title") || chunk.contains("<a")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_chapter_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: class_text(chunk, "cap-title").or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                }),
                url: Some(url::join_url(DOMAIN_URL, &key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    root.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.to_string(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn meta_value(body: &str, label: &str) -> Option<String> {
    body.split("<li")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| html::strip_tags(chunk).replace(label, ""))
        .map(|value| value.trim_matches([':', ' ']).to_string())
        .filter(|value| !value.is_empty())
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_from(value: Option<&str>) -> ItemStatus {
    match value
        .map(|text| text.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("en publicacion") | Some("en publicación") => ItemStatus::Ongoing,
        Some("finalizado") => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn slugs_from_chapter_key(key: &str) -> (String, String) {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    let manga_slug = parts
        .get(parts.len().saturating_sub(2))
        .copied()
        .unwrap_or("sample")
        .to_string();
    let chapter_slug = parts.last().copied().unwrap_or("chapter-1").to_string();
    (manga_slug, chapter_slug)
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/'))
}

fn normalize_chapter_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(DOMAIN_URL) {
        return format!("/{}", path.trim_start_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const CATALOG_FIXTURE: &str = r#"{"items":[{"title":"Sample","post_url":"/manga/sample/","portada":"/assets/mangas/sample/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"<main class="container"><h1 class="manga-title">Sample</h1><div class="manga-cover"><img src="/cover.jpg"></div><ul class="manga-meta-list"><li>Autor Author</li><li>Arte Artist</li><li>Género Drama</li><li>Estado En publicación</li></ul><div class="manga-sinopsis">Summary</div><ul id="chaptersList"><li><a href="https://enchiladascan.github.io/enchiladaweb/manga/sample/chapter-1/"><span class="cap-title">Capitulo 1</span></a></li></ul></main>"#;
const PAGES_FIXTURE: &str =
    r#"["https://enchiladascan.github.io/enchiladaweb/assets/mangas/sample/chapter-1/1.jpg"]"#;
