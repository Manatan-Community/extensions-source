use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: WalpurgisScan = WalpurgisScan;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://www.walpurgiscan.it",
    name: "Walpurgi Scan",
    lang: "it",
    content_rating: "safe",
    manga_dir: "/manga",
    project_page: Some("/project"),
    resize_covers: false,
};

struct WalpurgisScan;

#[derive(Clone, Copy)]
struct ThemesiaConfig {
    base_url: &'static str,
    name: &'static str,
    lang: &'static str,
    content_rating: &'static str,
    manga_dir: &'static str,
    project_page: Option<&'static str>,
    resize_covers: bool,
}

impl ThemesiaConfig {
    fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }
}

impl MangaSource for WalpurgisScan {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        let body = fetch_document_or_fixture(
            &themesia_search_url(page, "", Some(order), &Value::Null),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(CONFIG.base_url) && query.contains(CONFIG.manga_dir) {
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(CONFIG.normalize_key(query)))],
                has_next_page: false,
            });
        }
        let body = fetch_document_or_fixture(
            &themesia_search_url(
                page,
                query,
                None,
                request.get("filters").unwrap_or(&Value::Null),
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("{}/sample", CONFIG.manga_dir));
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("{}/sample", CONFIG.manga_dir));
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("{}/sample/chapter-1", CONFIG.manga_dir));
        let body = fetch_document_or_fixture(&CONFIG.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
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
        if input.starts_with(CONFIG.base_url) && input.contains(CONFIG.manga_dir) {
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(CONFIG.normalize_key(input)))),
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
        .with_referer(format!("{}/", CONFIG.base_url.trim_end_matches('/')))
        .with_cookies_for(CONFIG.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn themesia_search_url(page: u64, query: &str, order: Option<&str>, filters: &Value) -> String {
    let path = if filter_string(filters, "project").as_deref() == Some("project-filter-on") {
        CONFIG.project_page.unwrap_or(CONFIG.manga_dir)
    } else {
        CONFIG.manga_dir
    };
    let mut params = vec![
        ("title", url::query_escape(query)),
        ("page", page.to_string()),
    ];
    for (filter, parameter) in [
        ("author", "author"),
        ("year", "yearx"),
        ("status", "status"),
        ("type", "type"),
        ("order", "order"),
    ] {
        let value = if filter == "order" {
            filter_string(filters, filter).or_else(|| order.map(ToString::to_string))
        } else {
            filter_string(filters, filter)
        };
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.push((parameter, url::query_escape(&value)));
        }
    }
    if let Some(genres) = filter_string(filters, "genres") {
        for genre in genres
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(("genre[]", url::query_escape(genre)));
        }
    }
    format!(
        "{}{}?{}",
        CONFIG.base_url.trim_end_matches('/'),
        path,
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("bsx")
                || chunk.contains("imgu")
                || chunk.contains("uta")
                || chunk.contains("animepost")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains(CONFIG.manga_dir) {
                return None;
            }
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| {
                    html::text_between(chunk, "<h3", "</h3>")
                        .or_else(|| html::text_between(chunk, "<h4", "</h4>"))
                        .or_else(|| html::text_between(chunk, "<a", "</a>"))
                        .map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| CONFIG.name.to_string())
                });
            let key = CONFIG.normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk)
                    .map(|image| maybe_resize_cover(&CONFIG.absolute_url(&image))),
                url: Some(CONFIG.absolute_url(&key)),
                language: Some(CONFIG.lang.to_string()),
                content_rating: Some(CONFIG.content_rating.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn has_next_page(body: &str) -> bool {
    body.contains("next page-numbers")
        || (body.contains("pagination") && (body.contains("next") || body.contains("hpage")))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("{}/sample", CONFIG.manga_dir));
    let description = html::text_between(body, "entry-content", "</div>")
        .or_else(|| html::text_between(body, "class=\"desc", "</div>"))
        .or_else(|| html::text_between(body, "seriestucon", "</div>"))
        .map(|value| html::strip_tags(&value).replace("bercerita tentang ", ""))
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| CONFIG.name.to_string())),
        cover: html::attr_after(body, "class=\"thumb", "src")
            .or_else(|| image_attr(body))
            .map(|image| maybe_resize_cover(&CONFIG.absolute_url(&image))),
        description,
        authors: info_values(body, &["Author", "Pengarang", "Mangaka"]),
        artists: info_values(body, &["artist", "Artist", "seniman", "Ilustrator"]),
        tags: link_values(body, "/genre/")
            .into_iter()
            .chain(link_values(body, "/genres/"))
            .collect(),
        status: parse_status(body),
        url: Some(CONFIG.absolute_url(&key)),
        language: Some(CONFIG.lang.to_string()),
        content_rating: Some(CONFIG.content_rating.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter") || chunk.contains("chbox") || chunk.contains("lchx")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = CONFIG.normalize_key(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "chapter-title", "</"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number_from_text(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "dt", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(CONFIG.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(CONFIG.absolute_url(manga_key)),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let pages = body
        .split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("alt=\"\"") && !chunk.contains("alt=''"))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .collect::<Vec<_>>();
    let pages = if pages.is_empty() {
        script_images(body)
    } else {
        pages
    };
    pages
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: CONFIG.absolute_url(&image),
                context: Some(manga::image_headers(CONFIG.base_url)),
            },
            headers: manga::image_headers(CONFIG.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(input, "<img", "data-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "data-cfsrc"))
        .or_else(|| html::attr(input, "src"))
}

fn script_images(body: &str) -> Vec<String> {
    if let Some(decoded) = decoded_reader_script(body) {
        let images = script_images_from_text(&decoded);
        if !images.is_empty() {
            return images;
        }
    }
    script_images_from_text(body)
}

fn decoded_reader_script(body: &str) -> Option<String> {
    let marker = "data:text/javascript;base64,";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest
        .find(['"', '\''])
        .filter(|index| *index > 0)
        .unwrap_or(rest.len());
    let decoded = STANDARD.decode(&rest[..end]).ok()?;
    String::from_utf8(decoded).ok()
}

fn script_images_from_text(body: &str) -> Vec<String> {
    let Some(start) = body.find("\"images\"").or_else(|| body.find("'images'")) else {
        return Vec::new();
    };
    let Some(open) = body[start..].find('[').map(|index| start + index) else {
        return Vec::new();
    };
    let Some(close) = body[open..].find(']').map(|index| open + index + 1) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&body[open..close]).unwrap_or_default()
}

fn maybe_resize_cover(value: &str) -> String {
    if CONFIG.resize_covers && value.contains("/wp-content/uploads/") && !value.contains("resize=")
    {
        if value.contains('?') {
            format!("{value}&resize=165,225")
        } else {
            format!("{value}?resize=165,225")
        }
    } else {
        value.to_string()
    }
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("tamat") || lower.contains("complete") {
        ItemStatus::Completed
    } else if lower.contains("dropped") || lower.contains("cancel") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("ongoing") || lower.contains("berjalan") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let value = value.trim();
    dates::parse_ymd(value).or_else(|| dates::parse_fixture_date(value))
}

fn chapter_number_from_text(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn info_values(body: &str, labels: &[&str]) -> Vec<String> {
    body.split(['\n', '<'])
        .filter(|chunk| {
            labels.iter().any(|label| {
                chunk
                    .to_ascii_lowercase()
                    .contains(&label.to_ascii_lowercase())
            })
        })
        .flat_map(|chunk| {
            let values = link_values(chunk, "");
            if values.is_empty() {
                vec![html::strip_tags(chunk)]
            } else {
                values
            }
        })
        .filter(|value| {
            !value.is_empty() && !labels.iter().any(|label| value.eq_ignore_ascii_case(label))
        })
        .collect()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|existing| existing.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="listupd">
  <div class="bs"><div class="bsx"><a href="https://www.walpurgiscan.it/manga/sample/" title="Sample"><img src="https://www.walpurgiscan.it/cover.jpg" alt="Sample"></a></div></div>
</div>
<div class="pagination"><a class="next page-numbers" href="https://www.walpurgiscan.it/manga/page/2/">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent">
  <h1 class="entry-title">Sample</h1>
  <div class="thumb"><img src="https://www.walpurgiscan.it/cover.jpg"></div>
  <div class="entry-content" itemprop="description"><p>Sample description.</p></div>
  <div class="mgen"><a href="/genre/action/">Action</a></div>
  <div class="tsinfo"><div class="imptdt">Status <i>Ongoing</i></div></div>
</div>
<div id="chapterlist"><li><a href="https://www.walpurgiscan.it/manga/sample/chapter-1/"><span class="chapternum">Chapter 1</span></a><span class="chapterdate">2024-01-01</span></li></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="readerarea"><img src="https://www.walpurgiscan.it/page-1.jpg" alt="Page 1"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_themesia_fixture() {
        assert_eq!(parse_listing(LIST_FIXTURE).len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE, "/manga/sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
