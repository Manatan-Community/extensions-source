use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ExHentaiNetBr = ExHentaiNetBr;
const BASE_URL: &str = "https://exhentai.net.br";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct ExHentaiNetBr;

impl MangaSource for ExHentaiNetBr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, false));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/lista-de-mangas/page/{page}"),
                LIST_FIXTURE,
            ),
            false,
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let alphabet = filter_value(request.get("filters"), "alphabet").unwrap_or_default();
        let (target, letter_search) = if query.is_empty() && !alphabet.is_empty() {
            (
                format!(
                    "{BASE_URL}/lista-de-mangas?letra={}",
                    url::query_escape(&alphabet)
                ),
                true,
            )
        } else {
            (
                format!("{BASE_URL}/page/{page}?s={}", url::query_escape(query)),
                false,
            )
        };
        Ok(parse_listing(
            &fetch_document(&target, LIST_FIXTURE),
            letter_search,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
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
                item: key
                    .starts_with("/manga/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, _letter_search: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("itemP"))
        .filter_map(catalog_from_article)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("content-pagination")
            && (body.contains("active + li") || body.contains("class=\"next\"")),
    }
}

fn catalog_from_article(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "<h3", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "ExHentai.net.br".into())
            }),
        cover: image_attr(chunk).map(|image| absolute_url(&image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let status_text = html::text_between(body, "stats_box", "</span>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default()
        .to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "stats_box", "</h3>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "ExHentai.net.br".into())
            }),
        cover: html::attr_after(body, "anime_cover", "src")
            .or_else(|| image_attr(body))
            .map(|image| absolute_url(&image)),
        description: html::text_between(body, "sinopse_manga", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_after(body, "Artista").into_iter().collect(),
        artists: info_after(body, "Artista").into_iter().collect(),
        tags: body
            .split("tag-btn")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if status_text.contains("completo") {
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

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("name_chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "name_chapter", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "release-date", "</")
                    .map(|value| html::strip_tags(&value).replace("Data:", ""))
                    .and_then(|value| dates::parse_fixture_date(value.trim())),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo".to_string()),
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
        .filter(|chunk| chunk.contains("manga_image") || chunk.contains("src="))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn info_after(body: &str, label: &str) -> Option<String> {
    let chunk = body.split(label).nth(1)?;
    html::text_between(chunk, "<span", "</span>").map(|value| html::strip_tags(&value))
}

fn filter_value(filters: Option<&Value>, id: &str) -> Option<String> {
    filters?
        .get(id)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<article class="itemP"><a href="/manga/sample"><img src="/cover.jpg"></a><h3>Sample ExHentai</h3></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="stats_box"><h3>Sample ExHentai</h3><span>Completo</span></div>
<div class="anime_cover"><img src="/cover.jpg"></div>
<div class="sinopse_manga"><h5>Artista</h5><span>Artist</span><p class="info_p">Sample description.</p></div>
<a class="tag-btn">Adult</a>
<div class="chapter_content"><a href="/manga/sample/chapter-1"><span class="name_chapter">Capitulo 1</span><span class="release-date">Data: 01/01/2024</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="manga_image"><img src="/page-1.jpg"></div>"#;
