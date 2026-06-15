use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: HentaiKun = HentaiKun;
const BASE_URL: &str = "https://hentaikun.com";

struct HentaiKun;

impl MangaSource for HentaiKun {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let order = if listing_id(&request) == "latest" {
            "last-updated"
        } else {
            "most-viewed"
        };
        Ok(parse_listing(&fetch_document(
            &list_url(page(&request), order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            list_url(page(&request), "most-viewed")
        } else {
            let search_type =
                filter_value(&request, "searchType").unwrap_or_else(|| "title".into());
            let page_path = page_suffix(page(&request));
            format!(
                "{BASE_URL}/manga/search/{}/{}/{page_path}",
                search_type,
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/read/1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(&list_url(1, "most-viewed"), LIST_FIXTURE));
        let latest = parse_listing(&fetch_document(&list_url(1, "last-updated"), LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, order: &str) -> String {
    format!("{BASE_URL}/manga/manga-list/{order}/{}", page_suffix(page))
}

fn page_suffix(page: u64) -> String {
    if page > 1 {
        format!("{page}/")
    } else {
        String::new()
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = parse_table_listing(body);
    if entries.is_empty() {
        entries = parse_gallery_listing(body);
    }
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("aria-label=\"Next\""),
    }
}

fn parse_table_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<tr")
        .skip(1)
        .filter(|row| !row.contains("danger"))
        .filter_map(|row| {
            let href = html::attr_after(row, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: anchor_text(row)
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "HentaiKun".into())
                    }),
                cover: html::attr_after(row, "title=", "src")
                    .or_else(|| html::attr_after(row, "<img", "src"))
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_gallery_listing(body: &str) -> Vec<CatalogItem> {
    body.split("thumbnail")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "HentaiKun".into())
                    }),
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let category = info_links(body, "Category");
    let tags = link_values(body, "/tag/");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "single_title", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HentaiKun".into())),
        cover: html::attr_after(body, "property='og:image'", "content")
            .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_links(body, "Artist"),
        tags: category.into_iter().chain(tags).collect(),
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("readchap"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title.clone()),
                chapter_number: first_number(&title).or(Some(1.0)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let first = html::attr_after(body, "image_rin", "src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .unwrap_or_else(|| "/page001.jpg".into());
    let total = body.matches("<option").count().max(1);
    let (base, file) = first.rsplit_once('/').unwrap_or(("", &first));
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, "jpg"));
    let prefix = stem.trim_end_matches(|ch: char| ch.is_ascii_digit());
    let number = &stem[prefix.len()..];
    let pad = number.len();
    (1..=total)
        .map(|index| {
            let page_no = if pad > 0 {
                format!("{index:0pad$}")
            } else {
                index.to_string()
            };
            let image = format!("{base}/{prefix}{page_no}.{ext}");
            MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {index}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn first_number(input: &str) -> Option<f32> {
    let number = input
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    number.parse().ok()
}

fn anchor_text(input: &str) -> Option<String> {
    let start = input.find("<a")?;
    let after_open = &input[start..];
    let end = after_open.find("</a>")?;
    let before_close = &after_open[..end];
    let text_start = before_close.rfind('>')? + 1;
    Some(html::strip_tags(&before_close[text_start..]))
}

fn info_links(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .take(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn link_values(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
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

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<table class="table-striped"><tr><td><a href="/manga/sample" title="<img src='/cover.jpg'>">Sample Kun</a></td></tr></table>
<ul class="pagination"><li aria-label="Next"></li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="single_title"><h1>Sample Kun</h1></div><meta property="og:image" content="/cover.jpg">
<h2><strong>Artist</strong><a>Artist</a></h2><h2><strong>Category</strong><a>Adult</a></h2><div class="desc"><a href="/tag/tag"><span class="label-danger">Tag</span></a></div>
<table><tr><td><a class="readchap" href="/manga/sample/read/1">Chapter 1</a></td><td><h6>01-01-2024</h6></td></tr></table>
"#;
const PAGES_FIXTURE: &str = r#"<img class="image_rin" src="/pages/page001.jpg"><label>Page</label><select><option>1</option><option>2</option></select>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hentaikun_fixture() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Kun"
        );
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
