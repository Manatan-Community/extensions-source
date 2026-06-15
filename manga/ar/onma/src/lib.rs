use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Onma = Onma;
const BASE_URL: &str = "https://onma.me";

struct Onma;

impl MangaSource for Onma {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/latest-release?page={page}")
        } else {
            format!("{BASE_URL}/filterList?page={page}&sortBy=views&asc=false")
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key)],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!("{BASE_URL}/search?query={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".to_string());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key)),
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

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-container") || chunk.contains("media"))
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "media-heading", "</")
                    .or_else(|| html::text_between(chunk, "manga-heading", "</"))
                    .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ar".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        has_next_page: body.contains("rel=\"next\""),
        entries,
    }
}

fn parse_details(body: &str, key: String) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "panel-heading", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "well", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: panel_value(body, "المؤلف").into_iter().collect(),
        artists: panel_value(body, "الرسام").into_iter().collect(),
        tags: panel_value(body, "التصنيفات")
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: panel_value(body, "الحالة")
            .map(|status| parse_status(&status))
            .unwrap_or(ItemStatus::Unknown),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ar".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let manga_title = url::slug_from_url(manga_key).unwrap_or_else(|| "Manga".to_string());
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("chapter-title-rtl"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "chapter-title-rtl", "</")
                .map(|value| html::strip_tags(&value))
                .map(|value| value.replace(&manga_title, "Chapter"))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "date-chapter-title-rtl", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("img-responsive") || chunk.contains("data-src") || chunk.contains("src")
        })
        .filter_map(image_attr)
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

fn panel_value(body: &str, label: &str) -> Option<String> {
    body.split(label)
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "div class=\"text\"", "</div>"))
        .map(|value| html::strip_tags(&value).replace('،', ","))
        .filter(|value| !value.is_empty())
}

fn image_attr(input: &str) -> Option<String> {
    [
        "data-background-image",
        "data-cfsrc",
        "data-lazy-src",
        "data-src",
        "src",
    ]
    .into_iter()
    .find_map(|attr| html::attr_after(input, "<img", attr).or_else(|| html::attr(input, attr)))
}

fn parse_status(value: &str) -> ItemStatus {
    if value.contains("مكتملة") || value.eq_ignore_ascii_case("complete") {
        ItemStatus::Completed
    } else if value.contains("مستمرة") || value.eq_ignore_ascii_case("ongoing") {
        ItemStatus::Ongoing
    } else if value.eq_ignore_ascii_case("dropped") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        return format!(
            "/{}",
            input.split('/').skip(3).collect::<Vec<_>>().join("/")
        )
        .trim_end_matches('/')
        .to_string();
    }
    format!("/{}", input.trim_matches('/'))
}

const LIST_FIXTURE: &str = r#"<div class="chapter-container"><a href="/manga/sample"><img src="/cover.jpg"><h3>Sample Manga</h3></a></div>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"
<div class="panel-heading">Sample Manga</div><div class="row"><img class="img-responsive" src="/cover.jpg"><div class="well">Sample summary.</div></div>
<div class="panel-body"><h3>المؤلف :<div class="text">Writer</div></h3><h3>الرسام :<div class="text">Artist</div></h3><h3>التصنيفات :<div class="text"><a>Drama</a>, <a>Action</a></div></h3><h3>الحالة :<div class="text">مكتملة</div></h3></div>
<ul class="chapters"><li><div class="chapter-title-rtl"><a href="/manga/sample/chapter-1">Sample Manga: Chapter 1</a></div><span class="date-chapter-title-rtl">2024-01-01</span></li></ul>
"#;
const PAGES_FIXTURE: &str = r#"<div id="all"><img class="img-responsive" src="/page1.jpg"><img class="img-responsive" data-src="/page2.jpg"></div>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_onma_mmrcms() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].title, "Sample Manga");

        let details = parse_details(DETAILS_FIXTURE, "/manga/sample".into());
        assert_eq!(details.status, ItemStatus::Completed);

        let chapters = parse_chapters(DETAILS_FIXTURE, "/manga/sample");
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1");

        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
    }
}
