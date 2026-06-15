use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MangaKawaii = MangaKawaii;
const BASE_URL: &str = "https://www.mangakawaii.io";
const CDN_URL: &str = "https://cdn2.mangakawaii.io";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct MangaKawaii;

impl MangaSource for MangaKawaii {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_popular(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: if latest {
                parse_latest(&body)
            } else {
                parse_popular(&body)
            },
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let search_url = format!(
            "{BASE_URL}/search?query={}&search_type=manga&page={page}",
            url::query_escape(query)
        );
        Ok(parse_search(&fetch_document_or_fixture(
            &search_url,
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/1".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn parse_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("hot-manga__item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "hot-manga__item-name", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Mangakawaii".into())
                });
            Some(catalog_item(&href, &title))
        })
        .collect()
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("section__list-group-left"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Mangakawaii".into())
                });
            Some(catalog_item(&href, &title))
        })
        .collect()
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("section__list-group-heading"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&href).unwrap_or_else(|| "Mangakawaii".into())
                    });
                Some(catalog_item(&href, &title))
            })
            .collect(),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn catalog_item(href: &str, title: &str) -> CatalogItem {
    let key = normalize_key(href);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover: Some(format!(
            "{CDN_URL}/uploads{}/cover/cover_250x350.jpg",
            key.trim_end_matches('/')
        )),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let mut description = html::text_between(body, "dd class=\"text-justify text-break", "</dd>")
        .or_else(|| html::text_between(body, "dd class='text-justify text-break", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let alt_names = body
        .split("<span")
        .skip(1)
        .filter(|chunk| chunk.contains("alternativeHeadline"))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</span>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !alt_names.is_empty() {
        let mut next = description.take().unwrap_or_default();
        if !next.is_empty() {
            next.push_str("\n\n");
        }
        next.push_str("Alternative Names: ");
        next.push_str(&alt_names.join(", "));
        description = Some(next);
    }
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "<title", "</title>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Mangakawaii".into())),
        cover: html::attr_after(body, "manga-view__header-image", "src")
            .map(|value| url::join_url(BASE_URL, &value))
            .or_else(|| {
                Some(format!(
                    "{CDN_URL}/uploads{}/cover/cover_250x350.jpg",
                    key.trim_end_matches('/')
                ))
            }),
        authors: link_texts(body, "author"),
        artists: link_texts(body, "artist"),
        tags: link_texts(body, "category"),
        status: match text_by_class(body, "badge bg-success text-uppercase").as_deref() {
            Some("En Cours") => ItemStatus::Ongoing,
            Some("Termine") | Some("Terminé") => ItemStatus::Completed,
            _ => ItemStatus::Unknown,
        },
        description,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let visible = body
        .split("<tr")
        .skip(1)
        .filter(|chunk| chunk.contains("volume-"))
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return Vec::new();
    }
    let mut chapters = visible
        .iter()
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "table__chapter", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "<span", "</span>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".into());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: text_by_class(chunk, "table__date")
                    .and_then(|value| parse_dot_date(&value)),
                url: Some(url::join_url(BASE_URL, &href)),
                language: Some(LANG.into()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.len() < visible.len() {
        chapters.clear();
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter_slug = js_var(body, "chapter_slug").unwrap_or_else(|| "1".into());
    let manga_slug = js_var(body, "oeuvre_slug").unwrap_or_else(|| "sample".into());
    let app_locale = js_var(body, "applocale").unwrap_or_else(|| "fr".into());
    let chapter_server = js_var(body, "chapter_server").unwrap_or_else(|| "cdn2".into());
    if let Some(json) = js_array(body, "pages") {
        let cdn = format!("https://{chapter_server}.mangakawaii.io");
        let pages = serde_json::from_str::<Vec<PageDto>>(&json).unwrap_or_default();
        return pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| {
                page_from_url(
                    page.image_url(&cdn, &manga_slug, &app_locale, &chapter_slug),
                    index,
                )
            })
            .collect();
    }
    body.split("\"page_image\"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').nth(1).map(ToString::to_string))
        .enumerate()
        .map(|(index, page_image)| {
            page_from_url(
                format!("{CDN_URL}/uploads/manga/{manga_slug}/chapters_{app_locale}/{chapter_slug}/{page_image}"),
                index,
            )
        })
        .collect()
}

fn page_from_url(image: String, index: usize) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

#[derive(Deserialize)]
struct PageDto {
    #[serde(rename = "page_image")]
    page_image: String,
    #[serde(default)]
    external: i64,
    #[serde(default, rename = "page_version")]
    page_version: i64,
}

impl PageDto {
    fn image_url(
        &self,
        cdn: &str,
        manga_slug: &str,
        app_locale: &str,
        chapter_slug: &str,
    ) -> String {
        if self.external == 1 {
            return self.page_image.clone();
        }
        let version = if self.page_version > 0 {
            format!("?{}", self.page_version)
        } else {
            String::new()
        };
        format!(
            "{cdn}/uploads/manga/{manga_slug}/chapters_{app_locale}/{chapter_slug}/{}{version}",
            self.page_image
        )
    }
}

fn link_texts(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn text_by_class(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_dot_date(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('.');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn js_var(body: &str, name: &str) -> Option<String> {
    let marker = format!("var {name} = \"");
    let rest = body.split(&marker).nth(1)?;
    Some(rest.split('"').next()?.to_string())
}

fn js_array(body: &str, name: &str) -> Option<String> {
    let marker = format!("var {name} = ");
    let rest = body.split(&marker).nth(1)?;
    let end = rest.find("];")?;
    Some(rest[..end + 1].to_string())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        )
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

const LIST_FIXTURE: &str = r#"
<a class="hot-manga__item" href="/manga/sample"><div class="hot-manga__item-caption"><div class="hot-manga__item-name">Sample</div></div></a>
<div class="section__list-group-left"><a href="/manga/latest" title="Latest">Latest</a></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="section__list-group-heading"><a href="/manga/sample">Sample</a></div>
<ul class="pagination"><a rel="next" href="?page=2">Next</a></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><div class="manga-view__header-image"><img src="/cover.jpg"></div>
<dd class="text-justify text-break">Summary</dd>
<a href="/author/sample">Author</a><a href="/artist/sample">Artist</a><a href="/category/action">Action</a>
<span class="badge bg-success text-uppercase">En Cours</span><span itemprop="name alternativeHeadline">Alt Sample</span>
<tr class="volume-1"><td class="table__chapter"><a href="/manga/sample/1"><span>Chapitre 1</span></a></td><td class="table__date">01.01.2024</td></tr>
"#;

const PAGES_FIXTURE: &str = r#"
<script>
var chapter_slug = "1";
var oeuvre_slug = "sample";
var applocale = "fr";
var chapter_server = "cdn2";
var pages = [{"page_image":"001.jpg","external":0,"page_version":123},{"page_image":"https://img.example/page.jpg","external":1}];
</script>
"#;
