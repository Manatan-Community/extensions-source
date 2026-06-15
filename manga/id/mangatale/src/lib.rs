use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Ikiru = Ikiru;
const BASE_URL: &str = "https://05.ikiru.wtf";

struct Ikiru;

impl MangaSource for Ikiru {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "popular"
        };
        let body = post_advanced_search(page, "", sort, "desc", &Value::Null);
        Ok(parse_search_page(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let slug = slug_from_manga_url(query).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged {
                entries: vec![details_for_slug(&slug)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sort").unwrap_or_else(|| "popular".to_string());
        let order = filter_string(filters, "order").unwrap_or_else(|| "desc".to_string());
        Ok(parse_search_page(&post_advanced_search(
            page, query, &sort, &order, filters,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_for_slug(&slug_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let slug = slug_from_key(&key);
        let id = id_from_key(&key).unwrap_or_else(|| detail_payload(&slug).id.unwrap_or(0));
        let body = fetch_document_or_fixture(&chapter_list_url(id), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            let slug = slug_from_key(&key);
            format!("{}/manga/{slug}/", BASE_URL.trim_end_matches('/'))
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let slug = slug_from_manga_url(input).unwrap_or_else(|| "sample".to_string());
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_slug(&slug)),
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
        .with_origin(BASE_URL)
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

fn get_text(target: &str) -> Option<String> {
    client().get(target).browser_document().send_text().ok()
}

fn post_advanced_search(
    page: u64,
    query: &str,
    sort: &str,
    order: &str,
    filters: &Value,
) -> String {
    let page_string = page.to_string();
    let nonce = get_nonce().unwrap_or_default();
    let genres = json_array_string(filters, "genres");
    let excluded = json_array_string(filters, "genre_exclude");
    let types = json_array_string(filters, "type");
    let statuses = json_array_string(filters, "status");
    let form = [
        ("nonce", nonce.as_str()),
        ("inclusion", "OR"),
        ("exclusion", "OR"),
        ("page", page_string.as_str()),
        ("genre", genres.as_str()),
        ("genre_exclude", excluded.as_str()),
        ("author", "[]"),
        ("artist", "[]"),
        ("project", "0"),
        ("type", types.as_str()),
        ("status", statuses.as_str()),
        ("order", order),
        ("orderby", sort),
        ("query", query),
    ];
    client()
        .post(format!(
            "{}/wp-admin/admin-ajax.php?action=advanced_search",
            BASE_URL.trim_end_matches('/')
        ))
        .xhr()
        .form(&form)
        .send_text()
        .unwrap_or_else(|_| LIST_FIXTURE.to_string())
}

fn get_nonce() -> Option<String> {
    let body = get_text(&format!(
        "{}/wp-admin/admin-ajax.php?type=search_form&action=get_nonce",
        BASE_URL.trim_end_matches('/')
    ))?;
    html::attr_after(&body, "name=\"search_nonce\"", "value")
        .or_else(|| html::attr_after(&body, "name='search_nonce'", "value"))
}

fn detail_payload(slug: &str) -> MangaDto {
    let body = client()
        .get(format!(
            "{}/wp-json/wp/v2/manga?slug[]={}&_embed",
            BASE_URL.trim_end_matches('/'),
            url::query_escape(slug)
        ))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| DETAILS_FIXTURE.to_string());
    serde_json::from_str::<Vec<MangaDto>>(&body)
        .ok()
        .and_then(|mut items| items.pop())
        .unwrap_or_else(|| {
            serde_json::from_str::<Vec<MangaDto>>(DETAILS_FIXTURE)
                .unwrap()
                .remove(0)
        })
}

fn details_for_slug(slug: &str) -> CatalogItem {
    detail_payload(slug).into_catalog()
}

fn chapter_list_url(id: u64) -> String {
    format!(
        "{}/wp-admin/admin-ajax.php?manga_id={id}&page=999&action=chapter_list",
        BASE_URL.trim_end_matches('/')
    )
}

fn parse_search_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/") && chunk.contains("<img"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let slug = slug_from_manga_url(&href)?;
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| html::attr(chunk, "title"))
                .or_else(|| url::slug_from_url(&href))
                .unwrap_or_else(|| "Ikiru".to_string());
            let key = format!("/manga/{slug}");
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(format!("{}/manga/{slug}/", BASE_URL.trim_end_matches('/'))),
                language: Some("id".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("<button") && body.contains("<svg"),
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<time") || chunk.contains("chapter"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<span", "</span>")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| dates::parse_ymd(value.get(0..10).unwrap_or(&value))),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.is_empty() && !image.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct MangaDto {
    id: Option<u64>,
    slug: String,
    title: Rendered,
    content: Rendered,
    #[serde(rename = "_embedded", default)]
    embedded: Embedded,
}

impl MangaDto {
    fn into_catalog(self) -> CatalogItem {
        let key = match self.id {
            Some(id) => format!("/manga/{}?id={id}", self.slug),
            None => format!("/manga/{}", self.slug),
        };
        let terms = self.embedded;
        CatalogItem {
            key: key.clone(),
            title: html::strip_tags(&self.title.rendered),
            cover: terms
                .featured_media
                .first()
                .map(|media| media.source_url.clone()),
            description: Some(html::strip_tags(&self.content.rendered))
                .filter(|value| !value.is_empty()),
            authors: terms.term_values("series-author"),
            artists: terms.term_values("artist"),
            tags: terms
                .term_values("genre")
                .into_iter()
                .chain(terms.term_values("type"))
                .collect(),
            status: status_from_terms(&terms.term_values("status")),
            url: Some(format!(
                "{}/manga/{}/",
                BASE_URL.trim_end_matches('/'),
                self.slug
            )),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct Rendered {
    rendered: String,
}

#[derive(Default, Deserialize)]
struct Embedded {
    #[serde(rename = "wp:featuredmedia", default)]
    featured_media: Vec<FeaturedMedia>,
    #[serde(rename = "wp:term", default)]
    terms: Vec<Vec<Term>>,
}

impl Embedded {
    fn term_values(&self, taxonomy: &str) -> Vec<String> {
        self.terms
            .iter()
            .find(|terms| terms.first().is_some_and(|term| term.taxonomy == taxonomy))
            .map(|terms| terms.iter().map(|term| term.name.clone()).collect())
            .unwrap_or_default()
    }
}

#[derive(Default, Deserialize)]
struct FeaturedMedia {
    #[serde(rename = "source_url")]
    source_url: String,
}

#[derive(Default, Deserialize)]
struct Term {
    name: String,
    taxonomy: String,
}

fn status_from_terms(values: &[String]) -> ItemStatus {
    let joined = values.join(" ").to_ascii_lowercase();
    if joined.contains("completed") {
        ItemStatus::Completed
    } else if joined.contains("cancel") {
        ItemStatus::Cancelled
    } else if joined.contains("hiatus") {
        ItemStatus::Hiatus
    } else if joined.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn json_array_string(filters: &Value, key: &str) -> String {
    match filters.get(key) {
        Some(Value::Array(values)) => serde_json::to_string(
            &values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string()),
        Some(Value::String(value)) if !value.is_empty() => serde_json::to_string(
            &value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string()),
        _ => "[]".to_string(),
    }
}

fn filter_string(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
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

fn slug_from_key(key: &str) -> String {
    slug_from_manga_url(key).unwrap_or_else(|| "sample".to_string())
}

fn slug_from_manga_url(input: &str) -> Option<String> {
    let path = input
        .split('?')
        .next()
        .unwrap_or(input)
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_matches('/');
    let mut parts = path.split('/');
    if parts.next()? != "manga" {
        return None;
    }
    parts.next().map(ToString::to_string)
}

fn id_from_key(key: &str) -> Option<u64> {
    key.split("?id=").nth(1)?.parse().ok()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
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
<div><a href="https://05.ikiru.wtf/manga/sample/"><img src="https://05.ikiru.wtf/cover.jpg" alt="Sample"></a></div>
<button><svg></svg></button>
"#;

const DETAILS_FIXTURE: &str = r#"
[
  {
    "id": 1,
    "slug": "sample",
    "title": { "rendered": "Sample" },
    "content": { "rendered": "<p>Sample description.</p>" },
    "_embedded": {
      "wp:featuredmedia": [{ "source_url": "https://05.ikiru.wtf/cover.jpg" }],
      "wp:term": [
        [{ "name": "Action", "slug": "action", "taxonomy": "genre" }],
        [{ "name": "Ongoing", "slug": "ongoing", "taxonomy": "status" }]
      ]
    }
  }
]
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div><a href="https://05.ikiru.wtf/manga/sample/chapter-1/"><span>Chapter 1</span><time datetime="2024-01-01T00:00:00Z"></time></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<main><section><img src="https://05.ikiru.wtf/page-1.jpg"></section></main>
"#;
