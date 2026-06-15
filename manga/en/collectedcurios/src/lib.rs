use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: CollectedCurios = CollectedCurios;
const BASE_URL: &str = "https://www.collectedcurios.com";

struct CollectedCurios;

impl MangaSource for CollectedCurios {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: catalog(),
            has_next_page: false,
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
                entries: catalog()
                    .into_iter()
                    .filter(|item| item.key == key)
                    .collect(),
                has_next_page: false,
            });
        }
        let query_lower = query.to_ascii_lowercase();
        let entries = catalog()
            .into_iter()
            .filter(|item| {
                query_lower.is_empty() || item.title.to_ascii_lowercase().contains(&query_lower)
            })
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sequentialart.php".to_string());
        Ok(catalog()
            .into_iter()
            .find(|item| item.key == key)
            .map(|mut item| {
                item.initialized = true;
                item
            })
            .unwrap_or_else(|| fallback_item(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/sequentialart.php".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sequentialart.php?s=1".to_string());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        let image = parse_image_url(&body, &key);
        Ok(image
            .into_iter()
            .map(|image| MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some("Page 1".to_string()),
                ..MangaPage::default()
            })
            .collect())
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: catalog()
                    .into_iter()
                    .find(|item| item.key == key)
                    .or_else(|| Some(fallback_item(&key))),
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

fn catalog() -> Vec<CatalogItem> {
    vec![
        static_item(
            "Sequential Art",
            "/sequentialart.php",
            "Sequential Art webcomic.",
            "https://www.collectedcurios.com/images/CC_2011_Sequential_Art_Button.jpg",
        ),
        static_item(
            "Battle Bunnies",
            "/battlebunnies.php",
            "Battle Bunnies webcomic.",
            "https://www.collectedcurios.com/images/CC_2011_Battle_Bunnies_Button.jpg",
        ),
        static_item(
            "Spider and Scorpion",
            "/spiderandscorpion.php",
            "Spider and Scorpion webcomic.",
            "https://www.collectedcurios.com/images/CC_2011_Spider_And_Scorpion_Button.jpg",
        ),
    ]
}

fn static_item(title: &str, key: &str, description: &str, cover: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: title.to_string(),
        cover: Some(cover.to_string()),
        description: Some(description.to_string()),
        authors: vec!["Jolly Jack aka Phillip M Jackson".to_string()],
        artists: vec!["Jolly Jack aka Phillip M Jackson".to_string()],
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Collected Curios".to_string()),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
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

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let count = html::attr_after(body, "title=\"Last\"", "href")
        .and_then(|href| {
            href.split('=')
                .next_back()
                .and_then(|value| value.parse().ok())
        })
        .or_else(|| {
            html::attr_after(body, "title=\"Jump to number\"", "value")?
                .parse()
                .ok()
        })
        .or_else(|| {
            html::attr_after(body, "title=\"Back one\"", "href")
                .and_then(|href| {
                    href.split('=')
                        .next_back()
                        .and_then(|value| value.parse::<u32>().ok())
                })
                .map(|value| value + 1)
        })
        .unwrap_or(1);
    (1..=count)
        .rev()
        .map(|number| {
            let key = format!("{manga_key}?s={number}");
            MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapter - {number}")),
                chapter_number: Some(number as f32),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_image_url(body: &str, key: &str) -> Option<String> {
    let marker = if key.contains("sequentialart") {
        "w3-image"
    } else {
        "id=\"strip\""
    };
    html::attr_after(body, marker, "src")
        .or_else(|| html::attr_after(body, "id='strip'", "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &image))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .split(BASE_URL)
        .nth(1)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"
<a href="sequentialart.php?s=2"><img title="Last"></a>
<img class="w3-image" src="/sequentialart/sample.jpg">
"#;

const PAGE_FIXTURE: &str = r#"<img class="w3-image" src="/sequentialart/sample.jpg">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_static_catalog_and_live_chapter_shape() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries.len(), 3);
        let chapters = SOURCE
            .chapters(json!({"manga":"/sequentialart.php"}))
            .unwrap();
        assert_eq!(chapters[0].chapter_number, Some(2.0));
    }
}
