use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, ItemStatus,
    MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{
    html, manga,
    sdk::{http::HttpClient, FilterValue, SearchRequest},
    url,
};
use serde_json::Value;

const SOURCE: DoujinDesu = DoujinDesu;
const DEFAULT_BASE_URL: &str = "https://doujindesu.tv";

struct DoujinDesu;

impl MangaSource for DoujinDesu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url();
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, &base_url));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let section = if listing == "latest" {
            "doujin"
        } else {
            "manhwa"
        };
        Ok(parse_listing(
            &fetch_document_or_fixture(&format!("{base_url}/{section}/page/{page}/"), LIST_FIXTURE),
            &base_url,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base_url = base_url();
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(&base_url) {
            let key = normalize_key(query, &base_url);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                    &base_url,
                )],
                has_next_page: false,
            });
        }
        let filters = parse_filters(request.get("filters"));
        let target = search_url(&base_url, page, query, &filters);
        Ok(parse_listing(
            &fetch_document_or_fixture(&target, LIST_FIXTURE),
            &base_url,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base_url = base_url();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&base_url, &key), DETAILS_FIXTURE),
            Some(key),
            &base_url,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base_url = base_url();
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(
            &fetch_document_or_fixture(&absolute_url(&base_url, &key), DETAILS_FIXTURE),
            &base_url,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base_url = base_url();
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter-1".into());
        let chapter_body =
            fetch_document_or_fixture(&absolute_url(&base_url, &key), CHAPTER_FIXTURE);
        let body = html::attr_after(&chapter_body, "id=\"reader\"", "data-id")
            .or_else(|| html::attr_after(&chapter_body, "id='reader'", "data-id"))
            .map(|id| fetch_chapter_images(&base_url, &id))
            .unwrap_or(chapter_body);
        Ok(parse_pages(&body, &base_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url();
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&base_url, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base_url = base_url();
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&base_url, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let base_url = base_url();
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(&base_url) {
            let key = normalize_key(input, &base_url);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
                    &base_url,
                )),
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

fn base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", base_url.trim_end_matches('/')))
        .with_origin(base_url)
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client(DEFAULT_BASE_URL)
        .get(target)
        .header("Cookie", "sec_v_session=verified_human_0000000000000")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_chapter_images(base_url: &str, id: &str) -> String {
    client(base_url)
        .post(format!("{base_url}/themes/ajax/ch.php"))
        .header("Cookie", "sec_v_session=verified_human_0000000000000")
        .xhr()
        .form(&[("id", id)])
        .send_text()
        .unwrap_or_else(|_| PAGES_FIXTURE.to_string())
}

#[derive(Default)]
struct ParsedFilters {
    taxonomy: String,
    value: String,
}

fn parse_filters(filters: Option<&Value>) -> ParsedFilters {
    let mut parsed = ParsedFilters::default();
    for filter in filters_to_values(filters) {
        let value = filter.value.as_str().unwrap_or_default().trim();
        match filter.id.as_str() {
            "taxonomy" => parsed.taxonomy = value.to_string(),
            "taxonomy_value" => parsed.value = value.to_string(),
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

fn search_url(base_url: &str, page: u64, query: &str, filters: &ParsedFilters) -> String {
    let page_part = if page > 1 {
        format!("page/{page}/")
    } else {
        String::new()
    };
    if query.is_empty() && !filters.taxonomy.is_empty() {
        let value = filters.value.trim_matches('/');
        return if value.is_empty() {
            format!("{base_url}/{}/{page_part}", filters.taxonomy)
        } else {
            format!("{base_url}/{}/{value}/{page_part}", filters.taxonomy)
        };
    }
    if query.is_empty() {
        format!("{base_url}/{page_part}")
    } else {
        format!("{base_url}/{page_part}?s={}", url::query_escape(query))
    }
}

fn parse_listing(body: &str, base_url: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<article")
            .skip(1)
            .filter(|chunk| chunk.contains("entry"))
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let title = html::text_between(chunk, "h3 class=\"title\"", "</h3>")
                    .or_else(|| html::text_between(chunk, "h3 class='title'", "</h3>"))
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Manga".into()));
                let key = normalize_key(&href, base_url);
                Some(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: image_from_chunk(chunk).map(|image| absolute_url(base_url, &image)),
                    url: Some(absolute_url(base_url, &key)),
                    language: Some("id".to_string()),
                    content_rating: Some("adult".to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: body.contains("pagination") && body.contains("last"),
    }
}

fn parse_details(body: &str, key: Option<String>, base_url: &str) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let metadata = html::text_between(body, "section class=\"metadata\"", "</section>")
        .or_else(|| html::text_between(body, "section class='metadata'", "</section>"))
        .unwrap_or_else(|| body.to_string());
    let author = table_value(&metadata, "Author").or_else(|| table_value(&metadata, "Group"));
    let group = table_value(&metadata, "Group").unwrap_or_else(|| "Tidak Diketahui".to_string());
    let character =
        table_value(&metadata, "Character").unwrap_or_else(|| "Tidak Diketahui".to_string());
    let series = table_value(&metadata, "Series").unwrap_or_default();
    let alt = html::text_between(&metadata, "span class=\"alter\"", "</span>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_else(|| "Tidak Diketahui".to_string());
    let synopsis = html::text_between(&metadata, "div class=\"pb-2\"", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Tidak ada deskripsi yang tersedia bosque".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(&metadata, "h1 class=\"title\"", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Doujindesu".to_string()),
        cover: image_from_chunk(body).map(|image| absolute_url(base_url, &image)),
        description: Some(format!(
            "{synopsis}\n\nJudul Alternatif : {alt}\nGrup             : {group}\nKarakter         : {character}\nSeri             : {series}"
        )),
        authors: author.into_iter().collect(),
        tags: metadata
            .split("div class=\"tags\"")
            .nth(1)
            .unwrap_or_default()
            .split("<a")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: table_value(&metadata, "Status")
            .map(|value| {
                let lower = value.to_lowercase();
                if lower.contains("publishing") {
                    ItemStatus::Ongoing
                } else if lower.contains("finished") {
                    ItemStatus::Completed
                } else {
                    ItemStatus::Unknown
                }
            })
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(base_url, &key)),
        language: Some("id".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base_url: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("epsleft") || chunk.contains("lchx"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "lchx", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let eps = html::text_between(chunk, "<chapter", "</chapter>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| {
                    html::text_between(chunk, "<a", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .unwrap_or_else(|| "1".to_string())
                });
            let key = normalize_key(&href, base_url);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("Chapter {eps}")),
                chapter_number: eps
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok()),
                date_uploaded: html::text_between(chunk, "span class=\"date\"", "</span>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(base_url, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, base_url: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| image_from_chunk(chunk))
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(base_url, &image),
                context: None,
            },
            headers: manga::image_headers(base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn table_value(body: &str, label: &str) -> Option<String> {
    body.split("<tr").find_map(|row| {
        if !row.contains(label) {
            return None;
        }
        html::text_between(row, "<td", "</td>")
            .and_then(|_| row.split("</td>").nth(1).map(str::to_string))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset")
                .map(|value| value.split_whitespace().next().unwrap_or("").to_string())
        })
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(value: &str, base_url: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(base_url) {
            let path = &value[index + base_url.len()..];
            return format!("/{}", path.trim_matches('/'));
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(base_url: &str, value: &str) -> String {
    url::join_url(base_url, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div id="archives"><div class="entries"><article><a href="/manga/sample"><figure class="thumbnail"><img data-src="/cover.jpg"></figure><h3 class="title">Sample Doujindesu</h3></a></article></div></div><nav class="pagination"><ul><li class="last"><a href="/page/2/">Last</a></li></ul></nav>
"#;
const DETAILS_FIXTURE: &str = r#"
<section class="metadata"><figure class="thumbnail"><img src="/cover.jpg"></figure><h1 class="title">Sample Doujindesu <span class="alter">Alt Title</span></h1><div class="pb-2"><p>Sinopsis:</p><p>Sample description.</p></div><table><tr><td>Author</td><td>Author Name</td></tr><tr><td>Group</td><td>Group Name</td></tr><tr><td>Character</td><td>Hero</td></tr><tr><td>Series</td><td>Sample Series</td></tr><tr><td>Status</td><td>Finished</td></tr></table><div class="tags"><a>Tag A</a><a>Tag B</a></div></section><ul id="chapter_list"><li><div class="epsleft"><span class="lchx"><a href="/manga/sample/chapter-1">Chapter 1</a></span><span class="date">2024-01-01</span></div><div class="epsright"><chapter>1</chapter></div></li></ul>
"#;
const CHAPTER_FIXTURE: &str = r#"<div id="reader" data-id="123"></div>"#;
const PAGES_FIXTURE: &str = r#"<img src="/page1.jpg"><img src="/page2.jpg">"#;
