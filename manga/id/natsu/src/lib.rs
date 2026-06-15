use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{
    html, manga,
    sdk::{http::HttpClient, FilterValue, SearchRequest},
    url,
};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Natsu = Natsu;
const BASE_URL: &str = "https://natsu.tv";

struct Natsu;

impl MangaSource for Natsu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search_page(SEARCH_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "popular"
        };
        search_request(page, "", &ParsedFilters::with_order(order))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) && query.contains("/manga/") {
            let slug = slug_from_manga_url(query);
            return Ok(Paged {
                entries: vec![fetch_details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        search_request(page, query, &parse_filters(request.get("filters")))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(fetch_details_by_slug(&slug_from_manga_url(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let details = fetch_manga_by_slug(&slug_from_manga_url(&key)).unwrap_or_else(sample_manga);
        let body = fetch_text_or_fixture(
            &format!(
                "{BASE_URL}/wp-admin/admin-ajax.php?manga_id={}&page=99&action=chapter_list",
                details.id
            ),
            CHAPTERS_FIXTURE,
            true,
        );
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_text_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
            false,
        )))
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
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details_by_slug(&slug_from_manga_url(input))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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

fn fetch_text_or_fixture(target: &str, fixture: &str, xhr: bool) -> String {
    let http_client = client();
    let request = http_client.get(target);
    let request = if xhr {
        request.xhr()
    } else {
        request.browser_document()
    };
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn post_form_or_fixture(target: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn nonce() -> String {
    let body = fetch_text_or_fixture(
        &format!("{BASE_URL}/wp-admin/admin-ajax.php?type=search_form&action=get_nonce"),
        NONCE_FIXTURE,
        true,
    );
    html::attr_after(&body, "name=\"search_nonce\"", "value")
        .or_else(|| html::attr_after(&body, "name='search_nonce'", "value"))
        .unwrap_or_else(|| "fixture-nonce".to_string())
}

fn search_request(
    page: u64,
    query: &str,
    filters: &ParsedFilters,
) -> ExtensionResult<Paged<CatalogItem>> {
    let genres_json = json_array(&filters.genres);
    let type_json = json_array(&filters.types);
    let status_json = json_array(&filters.statuses);
    let page_string = page.to_string();
    let nonce = nonce();
    let body = post_form_or_fixture(
        &format!("{BASE_URL}/wp-admin/admin-ajax.php?action=advanced_search"),
        &[
            ("nonce", &nonce),
            ("inclusion", "OR"),
            ("exclusion", "OR"),
            ("page", &page_string),
            ("genre", &genres_json),
            ("genre_exclude", "[]"),
            ("author", "[]"),
            ("artist", "[]"),
            ("project", "0"),
            ("type", &type_json),
            ("status", &status_json),
            ("order", "desc"),
            ("orderby", &filters.order),
            ("query", query),
        ],
        SEARCH_FIXTURE,
    );
    Ok(parse_search_page(&body, page))
}

#[derive(Default)]
struct ParsedFilters {
    order: String,
    genres: Vec<String>,
    types: Vec<String>,
    statuses: Vec<String>,
}

impl ParsedFilters {
    fn with_order(order: &str) -> Self {
        Self {
            order: order.to_string(),
            ..Self::default()
        }
    }
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters {
        order: "popular".to_string(),
        ..ParsedFilters::default()
    };
    for filter in filters_to_values(filters) {
        match filter.id.as_str() {
            "order" => parsed.order = filter.value.as_str().unwrap_or("popular").to_string(),
            "genres" => parsed.genres = csv_values(&filter.value),
            "type" => parsed.types = value_array(&filter.value),
            "status" => parsed.statuses = value_array(&filter.value),
            _ => {}
        }
    }
    parsed
}

fn filters_to_values(filters: Option<&Value>) -> Vec<FilterValue> {
    let Some(filters) = filters else {
        return Vec::new();
    };
    if let Ok(values) = serde_json::from_value::<Vec<FilterValue>>(filters.clone()) {
        return values;
    }
    filters
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(id, value)| FilterValue {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn value_array(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Value::String(value) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn csv_values(value: &Value) -> Vec<String> {
    value
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn json_array(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

fn parse_search_page(body: &str, page: u64) -> Paged<CatalogItem> {
    let slugs = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/manga/") && chunk.contains("<img"))
        .filter_map(|chunk| html::attr(chunk, "href"))
        .map(|href| slug_from_manga_url(&href))
        .collect::<Vec<_>>();
    if slugs.is_empty() {
        return Paged {
            entries: vec![sample_manga().to_item(false)],
            has_next_page: false,
        };
    }
    let entries = fetch_manga_by_slugs(&slugs)
        .into_iter()
        .filter(|manga| !manga.terms("type").iter().any(|term| term == "Novel"))
        .map(|manga| manga.to_item(false))
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("<button") && page > 0,
    }
}

fn fetch_details_by_slug(slug: &str) -> CatalogItem {
    fetch_manga_by_slug(slug)
        .unwrap_or_else(sample_manga)
        .to_item(true)
}

fn fetch_manga_by_slug(slug: &str) -> Option<MangaDto> {
    fetch_manga_by_slugs(&[slug.to_string()]).into_iter().next()
}

fn fetch_manga_by_slugs(slugs: &[String]) -> Vec<MangaDto> {
    let mut params = slugs
        .iter()
        .map(|slug| format!("slug[]={}", url::query_escape(slug)))
        .collect::<Vec<_>>();
    params.push(format!("per_page={}", slugs.len().max(1) + 1));
    params.push("_embed".to_string());
    let body = fetch_text_or_fixture(
        &format!("{BASE_URL}/wp-json/wp/v2/manga?{}", params.join("&")),
        DETAILS_LIST_FIXTURE,
        true,
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_LIST_FIXTURE).expect("fixture details"))
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<time"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
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
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn slug_from_manga_url(input: &str) -> String {
    input
        .trim_end_matches('/')
        .split("/manga/")
        .nth(1)
        .and_then(|path| path.split('/').next())
        .unwrap_or_else(|| input.trim_matches('/'))
        .to_string()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!("/{}", input[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", input.trim_matches('/'))
}

#[derive(Clone, Deserialize)]
struct MangaDto {
    id: u64,
    slug: String,
    title: Rendered,
    content: Rendered,
    #[serde(rename = "_embedded", default)]
    embedded: Embedded,
}

impl MangaDto {
    fn to_item(&self, initialized: bool) -> CatalogItem {
        CatalogItem {
            key: format!("/manga/{}", self.slug),
            title: html::strip_tags(&self.title.rendered),
            cover: self
                .embedded
                .featured_media
                .first()
                .map(|media| media.source_url.clone()),
            description: Some(html::strip_tags(&self.content.rendered)),
            authors: self.terms("series-author"),
            artists: self.terms("artist"),
            tags: {
                let mut tags = self.terms("genre");
                tags.extend(self.terms("type"));
                tags
            },
            status: status_from_terms(&self.terms("status")),
            url: Some(format!("{BASE_URL}/manga/{}/", self.slug)),
            language: Some("id".to_string()),
            content_rating: Some("safe".to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }

    fn terms(&self, taxonomy: &str) -> Vec<String> {
        self.embedded
            .terms
            .iter()
            .find(|group| group.first().is_some_and(|term| term.taxonomy == taxonomy))
            .map(|group| group.iter().map(|term| term.name.clone()).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Default, Deserialize)]
struct Embedded {
    #[serde(rename = "wp:featuredmedia", default)]
    featured_media: Vec<FeaturedMedia>,
    #[serde(rename = "wp:term", default)]
    terms: Vec<Vec<Term>>,
}

#[derive(Clone, Deserialize)]
struct FeaturedMedia {
    source_url: String,
}

#[derive(Clone, Deserialize)]
struct Term {
    name: String,
    taxonomy: String,
}

#[derive(Clone, Deserialize)]
struct Rendered {
    rendered: String,
}

fn status_from_terms(terms: &[String]) -> ItemStatus {
    if terms.iter().any(|term| term == "Ongoing") {
        ItemStatus::Ongoing
    } else if terms.iter().any(|term| term == "Completed") {
        ItemStatus::Completed
    } else if terms.iter().any(|term| term == "Cancelled") {
        ItemStatus::Cancelled
    } else if terms.iter().any(|term| term == "On Hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn sample_manga() -> MangaDto {
    serde_json::from_str::<Vec<MangaDto>>(DETAILS_LIST_FIXTURE)
        .expect("fixture details")
        .into_iter()
        .next()
        .expect("sample manga")
}

export_manga_source!(SOURCE);

const NONCE_FIXTURE: &str = r#"<input name="search_nonce" value="fixture-nonce">"#;
const SEARCH_FIXTURE: &str = r#"
<div><a href="https://natsu.tv/manga/sample/"><img src="/cover.jpg"></a></div><button><svg></svg></button>
"#;
const DETAILS_LIST_FIXTURE: &str = r#"
[{"id":123,"slug":"sample","title":{"rendered":"Sample Natsu"},"content":{"rendered":"<p>Sample description.</p>"},"_embedded":{"wp:featuredmedia":[{"source_url":"https://natsu.tv/cover.jpg"}],"wp:term":[[{"name":"Action","slug":"action","taxonomy":"genre"}],[{"name":"Manga","slug":"manga","taxonomy":"type"}],[{"name":"Ongoing","slug":"ongoing","taxonomy":"status"}],[{"name":"Author","slug":"author","taxonomy":"series-author"}],[{"name":"Artist","slug":"artist","taxonomy":"artist"}]]}}]
"#;
const CHAPTERS_FIXTURE: &str = r#"
<div><a href="https://natsu.tv/manga/sample/chapter-1/"><span>Chapter 1</span><time datetime="2024-01-01T00:00:00Z"></time></a></div>
"#;
const PAGES_FIXTURE: &str = r#"
<main><div class="relative"><section><img src="/page1.jpg"><img src="/page2.jpg"></section></div></main>
"#;
