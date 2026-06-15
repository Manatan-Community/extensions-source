use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, manga::MadaraConfig, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: MonteTai = MonteTai;
const BASE_URL: &str = "https://montetaiscanlator.xyz";
const CONFIG: MadaraConfig = MadaraConfig {
    base_url: BASE_URL,
    lang: "pt-BR",
    content_rating: "safe",
    manga_path: "manga",
    popular_url_marker: "mt-manga-catalog-card__title",
    use_load_more: false,
    latest_enabled: true,
};

struct MonteTai;

impl MangaSource for MonteTai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "latest"
        } else {
            "views"
        };
        let body = fetch_document(&CONFIG.list_url(page, order), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document(&CONFIG.search_url(page, query), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: manga::Madara::has_next_page(&body, &CONFIG),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        let ajax = fetch_ajax_chapters(&body);
        if ajax.is_empty() {
            Ok(manga::Madara::parse_chapters(&body, &key, &CONFIG))
        } else {
            Ok(ajax)
        }
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let body = fetch_document(&CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(manga::Madara::parse_pages(&body, &CONFIG))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| CONFIG.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| CONFIG.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = CONFIG.normalize_manga_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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
    manga::Madara::browser_client(&CONFIG)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&CONFIG.absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("mt-manga-catalog-card"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "mt-manga-catalog-card__title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = CONFIG.normalize_manga_key(&href);
            let title = html::text_between(chunk, "mt-manga-catalog-card__title", "</")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Monte Tai".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| CONFIG.absolute_url(&image)),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some("pt-BR".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "post-title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Monte Tai".to_string())),
        cover: html::attr_after(body, "mtx-cover", "src")
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .map(|image| CONFIG.absolute_url(&image)),
        description: html::text_between(body, "mtx-synopsis", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: side_value(body, "Autor"),
        artists: side_value(body, "Artista"),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("mtx-chip") || chunk.contains("genre"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(body),
        url: Some(CONFIG.absolute_url(&key)),
        language: Some("pt-BR".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_ajax_chapters(details_body: &str) -> Vec<MangaChapter> {
    let Some(nonce) = details_body
        .find("nonce")
        .and_then(|start| details_body[start..].split('"').nth(2))
        .map(str::to_string)
    else {
        return Vec::new();
    };
    let Some(manga_id) = html::attr_after(details_body, "data-post", "data-post") else {
        return Vec::new();
    };
    let body = client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(&[
            ("action", "mt_get_summary_chapters"),
            ("nonce", &nonce),
            ("manga_id", &manga_id),
        ])
        .xhr()
        .send_text()
        .unwrap_or_default();
    parse_ajax_chapters(&body)
}

fn parse_ajax_chapters(body: &str) -> Vec<MangaChapter> {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    json.pointer("/data/rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let url = row.get("url").and_then(Value::as_str)?;
            let key = CONFIG.normalize_manga_key(url);
            Some(MangaChapter {
                key: key.clone(),
                title: row.get("title").and_then(Value::as_str).map(str::to_string),
                date_uploaded: row
                    .get("meta")
                    .and_then(Value::as_str)
                    .and_then(dates::parse_fixture_date),
                url: Some(CONFIG.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn side_value(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .filter_map(|chunk| html::text_between(chunk, "mtx-side-value", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("conclu") || lower.contains("completo") {
        ItemStatus::Completed
    } else if lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("hiato") {
        ItemStatus::Hiatus
    } else if lower.contains("andamento") || lower.contains("ativo") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="mt-manga-catalog-card"><a class="mt-manga-catalog-card__title" href="/manga/sample/">Sample</a><img src="/cover.jpg"></div><div class="nav-previous"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div class="mtx-cover"><img src="/cover.jpg"></div><div class="mtx-synopsis">Sample description.</div><div class="mtx-side-item">Autor <span class="mtx-side-value">Author</span></div><a data-post="123"></a><script id="mt-header-js-js-extra">var nonce=\"abc\";</script><ul><li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;
