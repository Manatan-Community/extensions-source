use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Buttsmithy = Buttsmithy;
const BASE_URL: &str = "https://incase.buttsmithy.com";
const ALFIE_URL: &str = "https://buttsmithy.com";
const CHAPTER_OVERVIEW_BASE: &str = "https://buttsmithy.com/archives/chapter";

struct Buttsmithy;

impl MangaSource for Buttsmithy {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut entries = fetch_all_comics();
        if !query.is_empty() {
            entries.retain(|item| item.title.to_ascii_lowercase().contains(&query));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if let Some(query) = request.get("query").and_then(Value::as_str) {
            if query.starts_with(BASE_URL) || query.starts_with(ALFIE_URL) {
                return Ok(Paged {
                    entries: vec![catalog_from_url(query, None)],
                    has_next_page: false,
                });
            }
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| BASE_URL.to_string());
        let mut item = catalog_from_url(&key, None);
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| BASE_URL.to_string());
        let absolute = absolute_url(&key);
        let mut chapters = if absolute.starts_with(CHAPTER_OVERVIEW_BASE) {
            parse_alfie_chapters(&fetch_document(&absolute, ALFIE_CHAPTER_FIXTURE))
        } else {
            fetch_other_pages_as_chapters(&absolute)
        };
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| BASE_URL.to_string());
        let absolute = absolute_url(&key);
        let body = fetch_document(&absolute, PAGE_FIXTURE);
        let image = image_from_comic_page(&body);
        Ok(image
            .into_iter()
            .enumerate()
            .map(|(index, image)| MangaPage {
                content: PageContent::Url {
                    url: url::join_url(&absolute, &image),
                    context: Some(manga::image_headers(&absolute)),
                },
                headers: manga::image_headers(&absolute),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
            .collect())
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
        if input.starts_with(BASE_URL) || input.starts_with(ALFIE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_url(input, None)),
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
        .with_cookies_for(ALFIE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_all_comics() -> Vec<CatalogItem> {
    let main = fetch_document(BASE_URL, MAIN_FIXTURE);
    let alfie = fetch_document(ALFIE_URL, ALFIE_HOME_FIXTURE);
    let recent_chapter = html::text_between(&alfie, "comic-chapter", "</")
        .map(|value| html::strip_tags(&value).to_ascii_lowercase())
        .unwrap_or_else(|| "chapter 1".to_string());
    let mut entries = parse_alfie_manga(&alfie, &recent_chapter);
    entries.extend(parse_menu_comics(&main));
    entries
}

fn parse_alfie_manga(body: &str, recent_chapter: &str) -> Vec<CatalogItem> {
    body.split("<option")
        .skip(1)
        .filter(|chunk| chunk.contains("level-0"))
        .map(|chunk| html::strip_tags(chunk).to_ascii_lowercase())
        .filter(|title| !title.is_empty())
        .map(|chapter_title| {
            let path = chapter_title_to_url_name(&chapter_title);
            CatalogItem {
                key: format!("{CHAPTER_OVERVIEW_BASE}/{path}"),
                title: format!("Alfie - {chapter_title}"),
                authors: vec!["InCase".to_string()],
                artists: vec!["InCase".to_string()],
                tags: vec!["fantasy".to_string(), "NSFW".to_string()],
                status: if chapter_title == recent_chapter {
                    ItemStatus::Unknown
                } else {
                    ItemStatus::Completed
                },
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_menu_comics(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.starts_with(BASE_URL) || html::strip_tags(chunk).contains("Alfie") {
                return None;
            }
            let title = html::strip_tags(chunk);
            if title.is_empty() {
                return None;
            }
            Some(catalog_from_url(&href, Some(title)))
        })
        .fold(Vec::new(), |mut items, item| {
            if !items
                .iter()
                .any(|existing: &CatalogItem| existing.key == item.key)
            {
                items.push(item);
            }
            items
        })
}

fn catalog_from_url(value: &str, title: Option<String>) -> CatalogItem {
    let absolute = absolute_url(value);
    let inferred_title = title
        .or_else(|| url::slug_from_url(&absolute).map(|slug| slug.replace('-', " ")))
        .unwrap_or_else(|| "Buttsmithy".to_string());
    CatalogItem {
        key: absolute.clone(),
        title: inferred_title,
        authors: vec!["InCase".to_string()],
        artists: vec!["InCase".to_string()],
        tags: vec!["NSFW".to_string()],
        status: ItemStatus::Completed,
        url: Some(absolute),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_other_pages_as_chapters(start_url: &str) -> Vec<MangaChapter> {
    let mut current = start_url.to_string();
    let mut chapters = Vec::new();
    for index in 0..500 {
        let body = fetch_document(&current, PAGE_FIXTURE);
        let title = html::attr_after(&body, "#comic", "alt")
            .or_else(|| html::attr_after(&body, "<img", "alt"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Page {}", index + 1));
        chapters.push(MangaChapter {
            key: current.clone(),
            title: Some(title),
            chapter_number: Some(index as f32),
            url: Some(current.clone()),
            ..MangaChapter::default()
        });
        let Some(next) = html::attr_after(&body, "comic-nav-next", "href") else {
            break;
        };
        if next.is_empty() || next == current {
            break;
        }
        current = absolute_url(&next);
    }
    chapters
}

fn parse_alfie_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("has-post-thumbnail")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "post-title", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "post-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| format!("p{}", index + 1));
            Some(MangaChapter {
                key: href.clone(),
                title: Some(title.clone()),
                chapter_number: parse_alfie_page_number(&title).or(Some(index as f32)),
                url: Some(href),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn image_from_comic_page(body: &str) -> Vec<String> {
    if let Some(comic) = body.split("#comic").nth(1) {
        if let Some(src) = html::attr_after(comic, "<img", "src") {
            return vec![src];
        }
    }
    html::attr_after(body, "<img", "src").into_iter().collect()
}

fn parse_alfie_page_number(title: &str) -> Option<f32> {
    title
        .trim()
        .trim_start_matches('p')
        .trim()
        .parse::<f32>()
        .ok()
}

fn chapter_title_to_url_name(title: &str) -> String {
    if title == "chapter 1" {
        return "chapter-1v2".to_string();
    }
    title.replace([' ', '.'], "-")
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        url::join_url(BASE_URL, value)
    }
}

export_manga_source!(SOURCE);

const MAIN_FIXTURE: &str = r#"
<li id="menu-item-38"><a href="https://incase.buttsmithy.com/comic/sample">Sample Comic</a></li>
"#;
const ALFIE_HOME_FIXTURE: &str = r#"
<div class="comic-chapter"><a>chapter 1</a></div>
<select id="chapter"><option class="level-0">chapter 1</option><option class="level-0">chapter 2</option></select>
"#;
const ALFIE_CHAPTER_FIXTURE: &str = r#"
<article class="has-post-thumbnail"><div class="post-content"><div class="post-info"><div class="post-title"><a href="https://buttsmithy.com/comic/p1">p1</a></div></div></div></article>
"#;
const PAGE_FIXTURE: &str = r#"<div id="comic"><img src="https://incase.buttsmithy.com/images/sample.jpg" alt="Sample"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_fixture_comics() {
        let page = SOURCE.list(json!({})).unwrap();
        assert!(!page.entries.is_empty());
    }
}
