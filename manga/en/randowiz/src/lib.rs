use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Randowiz = Randowiz;
const BASE_URL: &str = "https://randowis.com";

struct Randowiz;

impl MangaSource for Randowiz {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged { entries: series_items(), has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
        let entries = series_items()
            .into_iter()
            .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query))
            .collect();
        Ok(Paged { entries, has_next_page: false })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/category/we-live-in-an-mmo/".to_string());
        Ok(series_items().into_iter().find(|item| item.key == key).unwrap_or_else(|| series_items().remove(0)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/category/we-live-in-an-mmo/".to_string());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), CHAPTERS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample".to_string());
        Ok(parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE)))
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
        Ok(Some(UrlResolveResult {
            item: input.starts_with(BASE_URL).then(|| series_items().remove(0)),
            search: (!input.starts_with(BASE_URL)).then(|| SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn series_items() -> Vec<CatalogItem> {
    [
        (
            "Randowiz: We live in an MMO!?",
            "/category/we-live-in-an-mmo/",
            "The world of 'Mamuon' where players and NPC's live together in harmony. Or do they? DO THEY?",
            "https://i0.wp.com/randowis.com/wp-content/uploads/2016/02/MMO_CHP_001_CSP_000.jpg?resize=800%2C800&ssl=1",
        ),
        (
            "Randowiz: Short comics",
            "/category/short-comics/",
            "So short that i have to compensate..",
            "https://i0.wp.com/randowis.com/wp-content/uploads/2021/10/Images_PNGs_Site_BOT-SUPPORT.png",
        ),
        (
            "Randowiz: Illustations",
            "/category/art/",
            "You like draw? I give you draw.",
            "https://i0.wp.com/randowis.com/wp-content/uploads/2021/05/colour-studies-021-post.jpg",
        ),
    ]
    .into_iter()
    .map(|(title, key, description, cover)| CatalogItem {
        key: key.to_string(),
        title: title.to_string(),
        cover: Some(cover.to_string()),
        authors: vec!["Randowiz".to_string()],
        artists: vec!["Randowiz".to_string()],
        description: Some(description.to_string()),
        status: ItemStatus::Ongoing,
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
    .collect()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("has-post-thumbnail")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "elementor-post__title", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "elementor-post__title", "</")
                    .or_else(|| html::text_between(chunk, "<a", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    let len = chapters.len() as f32;
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some(len - index as f32);
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("elementor-widget-theme-post-content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: absolute_url(&image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

export_manga_source!(SOURCE);

const CHAPTERS_FIXTURE: &str = r#"<article class="has-post-thumbnail"><h2 class="elementor-post__title"><a href="/sample">Sample</a></h2></article>"#;
const PAGES_FIXTURE: &str = r#"<div class="elementor-widget-theme-post-content"><img src="/page1.jpg"></div>"#;
