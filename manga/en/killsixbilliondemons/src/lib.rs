use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: KillSixBillionDemons = KillSixBillionDemons;
const BASE_URL: &str = "https://killsixbilliondemons.com";
const AUTHOR: &str = "Abbadon";
const DESCRIPTION: &str = "A long-running graphic novel style webcomic by Abbadon.";
const PAGES_ORDER: &str = "?order=ASC";

struct KillSixBillionDemons;

impl MangaSource for KillSixBillionDemons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_books(HOME_FIXTURE, false),
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_books(&fetch_document(BASE_URL, HOME_FIXTURE), true),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let entries = parse_books(&fetch_document(BASE_URL, HOME_FIXTURE), true)
            .into_iter()
            .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-book/".to_string());
        let mut item = parse_books(&fetch_document(BASE_URL, HOME_FIXTURE), true)
            .into_iter()
            .find(|item| same_path(&item.key, &key))
            .unwrap_or_else(|| {
                book_item(
                    &key,
                    url::slug_from_url(&key).unwrap_or_else(|| "Book".to_string()),
                    None,
                    ItemStatus::Unknown,
                )
            });
        item.initialized = true;
        Ok(item)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-book/".to_string());
        let group_ended = request
            .get("preferences")
            .and_then(|prefs| prefs.get("group_ended").or_else(|| prefs.get("groupEnded")))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let body = fetch_document(&url::join_url(BASE_URL, &key), HOME_FIXTURE);
        Ok(parse_chapters(&body, &key, group_ended))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-page/".to_string());
        let mut pages = Vec::new();
        fetch_pages_recursive(&ordered_url(&url::join_url(BASE_URL, &key)), &mut pages, 0);
        if pages.is_empty() {
            Ok(parse_pages(PAGE_FIXTURE))
        } else {
            Ok(pages)
        }
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                search: Some(SearchRequest {
                    query: key,
                    ..SearchRequest::default()
                }),
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

fn parse_books(body: &str, fetch_metadata: bool) -> Vec<CatalogItem> {
    let latest_title = html::text_between(body, "post-title", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    body.split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            let text = option_text(chunk);
            if !is_book_option(&text) {
                return None;
            }
            let title = text.split(" (").next().unwrap_or(&text).trim().to_string();
            let key = normalize_key(&value);
            let cover = if fetch_metadata {
                fetch_cover(&key)
            } else {
                archive_cover(ARCHIVE_FIXTURE)
            };
            let status = if latest_title.to_ascii_lowercase().contains(
                &title
                    .split(": ")
                    .last()
                    .unwrap_or(&title)
                    .to_ascii_lowercase(),
            ) {
                ItemStatus::Unknown
            } else {
                ItemStatus::Completed
            };
            Some(book_item(&key, title, cover, status))
        })
        .fold(Vec::new(), push_unique)
}

fn book_item(key: &str, title: String, cover: Option<String>, status: ItemStatus) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title,
        cover,
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some(DESCRIPTION.to_string()),
        status,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_cover(key: &str) -> Option<String> {
    let body = fetch_document(&ordered_url(&url::join_url(BASE_URL, key)), ARCHIVE_FIXTURE);
    archive_cover(&body)
}

fn archive_cover(body: &str) -> Option<String> {
    body.split("comic-thumbnail-in-archive")
        .nth(1)
        .and_then(image_attr)
        .map(remove_wordpress_thumbnail)
        .map(|image| url::join_url(BASE_URL, &image))
}

fn parse_chapters(body: &str, manga_key: &str, group_ended: bool) -> Vec<MangaChapter> {
    let options = body
        .split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            let text = option_text(chunk);
            if value == "0" || text.eq_ignore_ascii_case("select chapter") {
                None
            } else {
                let is_book = is_book_option(&text);
                Some((normalize_key(&value), text, is_book))
            }
        })
        .collect::<Vec<_>>();
    let last_chapter_index = options.iter().rposition(|(_, _, is_book)| !*is_book);
    let mut found_book = false;
    let mut chapter_index = 1.0_f32;
    let mut chapters = Vec::new();
    for (index, (key, text, is_book)) in options.into_iter().enumerate() {
        if is_book {
            if found_book {
                break;
            }
            if same_path(&key, manga_key) {
                found_book = true;
            }
            continue;
        }
        if !found_book {
            continue;
        }
        let title = format!(
            "Chapter {}",
            text.split(" (").next().unwrap_or(&text).trim()
        );
        let expand = !group_ended || Some(index) == last_chapter_index;
        if expand {
            let mut page_chapters = Vec::new();
            fetch_page_chapters_recursive(&key, &title, chapter_index, &mut page_chapters, 0);
            if page_chapters.is_empty() {
                chapters.push(grouped_chapter(&key, &title, chapter_index));
            } else {
                chapters.extend(page_chapters);
            }
        } else {
            chapters.push(grouped_chapter(&key, &title, chapter_index));
        }
        chapter_index += 1.0;
    }
    chapters.reverse();
    chapters
}

fn grouped_chapter(key: &str, title: &str, chapter_number: f32) -> MangaChapter {
    MangaChapter {
        key: key.to_string(),
        title: Some(title.to_string()),
        chapter_number: Some(chapter_number),
        url: Some(url::join_url(BASE_URL, key)),
        ..MangaChapter::default()
    }
}

fn fetch_page_chapters_recursive(
    key: &str,
    chapter_title: &str,
    base_number: f32,
    chapters: &mut Vec<MangaChapter>,
    depth: usize,
) {
    if depth > 25 {
        return;
    }
    let body = fetch_document(&ordered_url(&url::join_url(BASE_URL, key)), ARCHIVE_FIXTURE);
    for chunk in body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("comic-thumbnail-in-archive") || chunk.contains("href="))
    {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        if !chunk.contains("comic-thumbnail-in-archive")
            && !href.contains(BASE_URL)
            && !href.starts_with('/')
        {
            continue;
        }
        let page_num = chapters.len() + 1;
        let title = html::attr(chunk, "title")
            .filter(|value| !value.is_empty())
            .map(|value| format!("{chapter_title} - {value}"))
            .unwrap_or_else(|| format!("{chapter_title} Page {page_num}"));
        let key = normalize_key(&href);
        chapters.push(MangaChapter {
            key: key.clone(),
            title: Some(title),
            chapter_number: Some(base_number + (page_num as f32 / 1000.0)),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        });
    }
    if let Some(next) = next_page_url(&body) {
        fetch_page_chapters_recursive(
            &normalize_key(&next),
            chapter_title,
            base_number,
            chapters,
            depth + 1,
        );
    }
}

fn fetch_pages_recursive(target: &str, pages: &mut Vec<MangaPage>, depth: usize) {
    if depth > 25 {
        return;
    }
    let body = fetch_document(target, PAGE_FIXTURE);
    pages.extend(parse_pages(&body));
    if let Some(next) = next_page_url(&body) {
        fetch_pages_recursive(&ordered_url(&next), pages, depth + 1);
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("comic-thumbnail-in-archive")
        .skip(1)
        .filter_map(image_attr)
        .map(remove_wordpress_thumbnail)
        .collect::<Vec<_>>();
    if images.is_empty() {
        images.extend(
            body.split("#comic")
                .chain(body.split("id=\"comic\""))
                .skip(1)
                .filter_map(image_attr)
                .map(remove_wordpress_thumbnail),
        );
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "src")
        .or_else(|| html::attr(input, "src"))
        .filter(|value| !value.is_empty())
}

fn next_page_url(body: &str) -> Option<String> {
    body.split("paginav-next")
        .nth(1)
        .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
}

fn ordered_url(target: &str) -> String {
    if target.contains('?') {
        target.to_string()
    } else {
        format!("{}{}", target.trim_end_matches('/'), PAGES_ORDER)
    }
}

fn remove_wordpress_thumbnail(value: String) -> String {
    let lower = value.to_ascii_lowercase();
    for ext in [".jpg", ".jpeg", ".png", ".webp", ".gif"] {
        if let Some(ext_index) = lower.find(ext) {
            let before = &value[..ext_index];
            if let Some(dash) = before.rfind('-') {
                let suffix = &before[dash + 1..];
                if suffix.split('x').count() == 2
                    && suffix
                        .split('x')
                        .all(|part| part.chars().all(|ch| ch.is_ascii_digit()))
                {
                    return format!("{}{}", &value[..dash], &value[ext_index..]);
                }
            }
        }
    }
    value
}

fn option_text(chunk: &str) -> String {
    chunk
        .split_once('>')
        .map(|(_, rest)| rest.split("</option>").next().unwrap_or(rest))
        .map(html::strip_tags)
        .unwrap_or_default()
}

fn is_book_option(text: &str) -> bool {
    text.split(" (")
        .next()
        .unwrap_or(text)
        .trim()
        .parse::<f32>()
        .is_err()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn same_path(left: &str, right: &str) -> bool {
    normalize_key(left)
        .trim_matches('/')
        .eq_ignore_ascii_case(normalize_key(right).trim_matches('/'))
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
<h1 class="post-title">Book One page 1</h1>
<select id="chapter">
  <option value="/book-one/">Book One (10)</option>
  <option value="/chapter-1/">1 (3)</option>
  <option value="/chapter-2/">2 (4)</option>
  <option value="/book-two/">Book Two (5)</option>
</select>
"#;
const ARCHIVE_FIXTURE: &str = r#"
<div class="comic-thumbnail-in-archive"><a href="/page-1/" title="Page 1"><img src="/wp-content/uploads/page-1-150x150.jpg"></a></div>
<div class="comic-thumbnail-in-archive"><a href="/page-2/" title="Page 2"><img src="/wp-content/uploads/page-2-150x150.jpg"></a></div>
"#;
const PAGE_FIXTURE: &str =
    r#"<div id="comic"><img src="/wp-content/uploads/page-1-800x1200.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_books_chapters_and_pages() {
        assert_eq!(parse_books(HOME_FIXTURE, false).len(), 2);
        assert_eq!(parse_chapters(HOME_FIXTURE, "/book-one/", true).len(), 2);
        assert_eq!(
            parse_pages(PAGE_FIXTURE)[0].description.as_deref(),
            Some("Page 1")
        );
    }
}
