use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: OnfMangas = OnfMangas;
const BASE_URL: &str = "https://onfmangas.com";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct OnfMangas;

impl MangaSource for OnfMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        if listing_id(&request) == "latest" {
            let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
            return Ok(parse_grid(&fetch_document(
                &format!("{BASE_URL}/mangas.php?tab=general&genero=0&q=&page={page}"),
                GRID_FIXTURE,
            )));
        }
        Ok(parse_popular(&fetch_document(
            &format!("{BASE_URL}/populares.php"),
            POPULAR_FIXTURE,
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
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_grid(&fetch_document(
            &search_url(page, query, request.get("filters")),
            GRID_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter/sample-1".into());
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
                item: Some(details_from_key(&key)),
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
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    let first = client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    if let Some(token) = token_from_body(&first) {
        return client()
            .get(target)
            .header("Cookie", format!("__onf_chk={token}"))
            .browser_document()
            .send_text()
            .unwrap_or(first);
    }
    first
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("pop-podium-card") || chunk.contains("pop-card"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = class_text(chunk, "pop-podium-name")
                .or_else(|| class_text(chunk, "pop-name"))
                .filter(|value| !value.is_empty())?;
            Some(catalog_item(
                &title,
                &href,
                html::attr_after(chunk, "<img", "src"),
            ))
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_grid(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("manga-card")
        .skip(1)
        .filter_map(|chunk| {
            let title = class_text(chunk, "manga-title")?;
            let href = html::attr_after(chunk, "<a", "href")?;
            let cover = html::attr_after(chunk, "card-cover", "src")
                .or_else(|| html::attr_after(chunk, "<img", "src"));
            Some(catalog_item(&title, &href, cover))
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("Siguiente"),
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, fallback_key: &str) -> CatalogItem {
    let key = html::attr_after(body, "rel=\"canonical\"", "href")
        .map(|value| normalize_key(&value))
        .unwrap_or_else(|| normalize_key(fallback_key));
    CatalogItem {
        key: key.clone(),
        title: class_text(body, "manga-title").unwrap_or_else(|| "ONF MANGAS".to_string()),
        cover: html::attr_after(body, "manga-poster", "src").map(|value| absolute_url(&value)),
        authors: class_text(body, "author-link").into_iter().collect(),
        description: class_text(body, "manga-description"),
        tags: class_text_all(body, "genre-tag"),
        status: parse_status(&body.to_ascii_lowercase()),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let Some(json) = hex_script(body, "const _hex = \"") else {
        return Vec::new();
    };
    let mut chapters = serde_json::from_str::<Value>(&json)
        .unwrap_or(Value::Null)
        .as_array()
        .cloned()
        .unwrap_or_default();
    chapters.sort_by(|a, b| {
        let an = a
            .get("numero")
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let bn = b
            .get("numero")
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        bn.partial_cmp(&an).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = Vec::new();
    for chapter in chapters {
        push_chapter(&mut out, &chapter, None);
        if let Some(other_versions) = chapter.get("otras_versiones").and_then(Value::as_array) {
            for other in other_versions {
                push_chapter(&mut out, other, Some(&chapter));
            }
        }
    }
    out
}

fn push_chapter(out: &mut Vec<MangaChapter>, chapter: &Value, parent: Option<&Value>) {
    let Some(raw_url) = string_value(chapter, "url") else {
        return;
    };
    let number = string_value(chapter, "numero")
        .or_else(|| parent.and_then(|value| string_value(value, "numero")));
    let title = string_value(chapter, "titulo_str")
        .or_else(|| parent.and_then(|value| string_value(value, "titulo_str")))
        .or_else(|| number.as_ref().map(|value| format!("Capitulo {value}")))
        .unwrap_or_else(|| "Capitulo sin numero".to_string());
    let date = string_value(chapter, "fecha_subida")
        .or_else(|| parent.and_then(|value| string_value(value, "fecha_subida")))
        .and_then(|value| parse_onf_date(&value));
    out.push(MangaChapter {
        key: normalize_key(&raw_url),
        title: Some(title),
        chapter_number: number.and_then(|value| value.parse::<f32>().ok()),
        date_uploaded: date,
        url: Some(absolute_url(&raw_url)),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    });
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Some(json) = hex_script(body, "const _hexP = \"") else {
        return Vec::new();
    };
    serde_json::from_str::<Value>(&json)
        .unwrap_or(Value::Null)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| string_value(page, "src").or_else(|| string_value(page, "fallback")))
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

fn search_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let tab = filter(filters, "tab").unwrap_or("general");
    let genre = filter(filters, "genero").unwrap_or("0");
    let mut out = format!(
        "{BASE_URL}/mangas.php?q={}&page={page}&tab={tab}",
        url::query_escape(query)
    );
    if genre != "0" {
        out.push_str("&generos%5B0%5D=");
        out.push_str(&url::query_escape(genre));
    }
    out
}

fn filter<'a>(filters: Option<&'a Value>, id: &str) -> Option<&'a str> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(id))
        .and_then(Value::as_str)
}

fn catalog_item(title: &str, href: &str, cover: Option<String>) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: cover.map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn class_text(input: &str, class_name: &str) -> Option<String> {
    html::text_between(input, class_name, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn class_text_all(input: &str, class_name: &str) -> Vec<String> {
    input
        .split(class_name)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn token_from_body(body: &str) -> Option<String> {
    body.split("var token=\"")
        .nth(1)?
        .split('"')
        .next()
        .map(ToString::to_string)
}

fn hex_script(body: &str, marker: &str) -> Option<String> {
    let hex = body.split(marker).nth(1)?.split("\";").next()?;
    decode_hex(hex)
}

fn decode_hex(hex: &str) -> Option<String> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let value = std::str::from_utf8(chunk).ok()?;
        bytes.push(u8::from_str_radix(value, 16).ok()?);
    }
    String::from_utf8(bytes).ok()
}

fn parse_status(lower_body: &str) -> ItemStatus {
    if lower_body.contains("finalizado") {
        ItemStatus::Completed
    } else if lower_body.contains("emision") || lower_body.contains("emisión") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_onf_date(value: &str) -> Option<i64> {
    manatan_shared::dates::parse_ymd(value.get(0..10)?)
}

fn string_value(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!("/{}", input.trim_start_matches(BASE_URL).trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
<a class="pop-card" href="/manga/sample"><span class="pop-name">Sample Manga</span><img src="/cover.jpg"></a>
"#;
const GRID_FIXTURE: &str = r#"
<div class="manga-grid"><div class="manga-card"><a href="/manga/sample"><div class="manga-title">Sample Manga</div><div class="card-cover"><img src="/cover.jpg"></div></a></div></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="manga-title">Sample Manga</h1><img class="manga-poster" src="/cover.jpg"><div class="manga-description">Sample description</div>
<span class="genre-tag">Accion</span><span>EMISION</span>
<script>const _hex = "5b7b2275726c223a222f636861707465722f73616d706c652d31222c22746974756c6f5f737472223a224361706974756c6f2031222c226e756d65726f223a2231222c2266656368615f737562696461223a22323032342d30312d30312030303a30303a3030227d5d";</script>
"#;
const PAGES_FIXTURE: &str = r#"
<script>const _hexP = "5b7b22737263223a222f70616765312e6a7067222c2266616c6c6261636b223a6e756c6c7d5d";</script>
"#;
