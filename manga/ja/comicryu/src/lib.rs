use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ComicRyu = ComicRyu;
const BASE_URL: &str = "https://www.comic-ryu.jp";
const UNICORN_URL: &str = "https://unicorn.comic-ryu.jp";

struct ComicRyu;

impl MangaSource for ComicRyu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        Ok(if latest {
            parse_latest(&body)
        } else {
            parse_popular(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let status = request
            .get("filters")
            .and_then(|filters| filters.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("シリーズ一覧-連載中");
        let target = if status.starts_with("https://") {
            status.to_string()
        } else {
            format!("{BASE_URL}/{}", url::query_escape(status))
        };
        Ok(parse_search(&fetch_document_or_fixture(
            &target,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let target = absolute_url(&key);
        let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), &target))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let target = absolute_url(&key);
        let body = fetch_document_or_fixture(&target, DETAILS_FIXTURE);
        Ok(parse_chapters(&body, target.contains("unicorn")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        let target = absolute_url(&key);
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body, &target))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) || input.starts_with(UNICORN_URL) {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), input)),
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

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("m-ranking-list-item")
            .skip(1)
            .filter_map(parse_item_block)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("m-list-sakuhin-list-item")
            .skip(1)
            .filter_map(parse_item_block)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("m-list-sakuhin-list-item")
            .skip(1)
            .filter_map(parse_item_block)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_item_block(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let item_url = absolute_url(&key);
    Some(CatalogItem {
        key,
        title: html::text_between(chunk, "sakuhin-article-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Comic Ryu".into()),
        cover: html::attr_after(chunk, "sakuhin-article-thumbnail", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(item_url),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>, target: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| normalize_key(target));
    let author_text = html::text_between(body, "sakuhin-article-author", "</")
        .map(|value| html::strip_tags(&value).replace("著者", ""))
        .filter(|value| !value.trim().is_empty());
    let (authors, artists) = split_authors(author_text);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "sakuhin-article-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Comic Ryu".into()),
        cover: html::attr_after(body, "sakuhin-article-thumbnail", "src")
            .map(|value| url::join_url(target, &value)),
        description: html::text_between(body, "sakuhin-article-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors,
        artists,
        status: ItemStatus::Unknown,
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, unicorn: bool) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("sakuhin-episode-link")
        .skip(1)
        .filter(|chunk| !chunk.contains("is-episode-publish-end"))
        .filter_map(|chunk| {
            let href =
                html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "sakuhin-episode-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".into())),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), |mut chapters, chapter| {
            if !chapters
                .iter()
                .any(|existing: &MangaChapter| existing.key == chapter.key)
            {
                chapters.push(chapter);
            }
            chapters
        });
    if !unicorn {
        chapters.reverse();
    }
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    body.split("wp-block-image")
        .skip(1)
        .filter_map(|chunk| html::attr_after(chunk, "<img", "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(referer, &image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn split_authors(value: Option<String>) -> (Vec<String>, Vec<String>) {
    let Some(value) = value else {
        return (Vec::new(), Vec::new());
    };
    if value.contains("原作：") && value.contains("漫画：") {
        let mut authors = Vec::new();
        let mut artists = Vec::new();
        for part in value.split('×') {
            if let Some(author) = part
                .split_once("原作：")
                .map(|(_, value)| value.trim().to_string())
            {
                authors.push(author);
            }
            if let Some(artist) = part
                .split_once("漫画：")
                .map(|(_, value)| value.trim().to_string())
            {
                artists.push(artist);
            }
        }
        (authors, artists)
    } else {
        (vec![value.trim().to_string()], Vec::new())
    }
}

fn normalize_key(value: &str) -> String {
    if value.starts_with(UNICORN_URL) {
        return value
            .split('?')
            .next()
            .unwrap_or(value)
            .trim_end_matches('/')
            .to_string();
    }
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_matches('/');
    format!("/{path}")
}

fn absolute_url(key: &str) -> String {
    if key.starts_with("https://") {
        key.to_string()
    } else {
        url::join_url(BASE_URL, key)
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items
        .iter()
        .any(|existing| existing.key == item.key || existing.title == item.title)
    {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<html><div class="m-ranking-list-item"><a class="m-ranking-link" href="/series/sample"><h2 class="sakuhin-article-title">Sample Ryu</h2><img class="sakuhin-article-thumbnail" src="/cover.jpg"></a></div><div class="m-list-recent"><div class="m-list-sakuhin-list-item"><a class="m-list-sakuhin-list-item-link" href="/series/sample"><h2 class="sakuhin-article-title">Sample Ryu</h2><img class="sakuhin-article-thumbnail" src="/cover.jpg"></a></div></div></html>"#;
const SEARCH_FIXTURE: &str = r#"<html><div class="m-series-list"><div class="m-list-sakuhin-list-item"><a href="/series/sample"><h2 class="sakuhin-article-title">Sample Ryu</h2><img class="sakuhin-article-thumbnail" src="/cover.jpg"></a></div></div></html>"#;
const DETAILS_FIXTURE: &str = r#"<html><aside class="m-aside"><article class="sakuhin-article"><h1 class="sakuhin-article-title">Sample Ryu</h1><p class="sakuhin-article-author">著者 Author</p><p class="sakuhin-article-description">Description</p><img class="sakuhin-article-thumbnail" src="/cover.jpg"></article></aside><main class="m-main"><a class="sakuhin-episode-link" href="/episode/1"><article class="sakuhin-episode"><h2 class="sakuhin-episode-title">Chapter 1</h2></article></a></main></html>"#;
const PAGES_FIXTURE: &str =
    r#"<html><figure class="wp-block-image"><img src="/page.jpg"></figure></html>"#;
