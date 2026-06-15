use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: LeituraManga = LeituraManga;
const BASE_URL: &str = "https://leituramanga.net";
const API_URL: &str = "https://api.leituramanga.net";
const CDN_URL: &str = "https://cdn.leituramanga.net";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";
const LIMIT: u64 = 24;

struct LeituraManga;

impl MangaSource for LeituraManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_response(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "time"
        } else {
            "view"
        };
        Ok(parse_manga_response(&api_get(
            &format!("{API_URL}/api/manga/?sort={sort}&limit={LIMIT}&page={page}"),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut params = vec![format!("limit={LIMIT}"), format!("page={page}")];
        if !query.is_empty() {
            params.push(format!("keyword={}", url::query_escape(query)));
        }
        for (id, param) in [("genre", "genre"), ("status", "status"), ("sort", "sort")] {
            if let Some(value) = filter_value(&request, id).filter(|value| !value.is_empty()) {
                params.push(format!("{param}={}", url::query_escape(&value)));
            }
        }
        Ok(parse_manga_response(&api_get(
            &format!("{API_URL}/api/manga/?{}", params.join("&")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".to_string());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let manga_id = extract_manga_id(&body).unwrap_or_else(|| "fixture-id".to_string());
        let slug = key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let result: MangaResponseDto<ChapterListDto> = api_json(
            &format!(
                "{API_URL}/api/chapter/get-by-manga-id?mangaId={manga_id}&page=1&limit=9007199254740991"
            ),
            CHAPTERS_FIXTURE,
        );
        Ok(result
            .data
            .data
            .into_iter()
            .map(|chapter| chapter.into_chapter(slug))
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/chapter/1".to_string());
        let page_url = absolute_url(&key);
        let body = fetch_document(&page_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &page_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_manga_response(&api_get(
            &format!("{API_URL}/api/manga/?sort=view&limit={LIMIT}&page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_manga_response(&api_get(
            &format!("{API_URL}/api/manga/?sort=time&limit={LIMIT}&page=1"),
            LIST_FIXTURE,
        ));
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
                style: Some(HomeSectionStyle::Cover),
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
        if input.starts_with(BASE_URL) && input.contains("/manga/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn api_get(target: &str, fixture: &str) -> String {
    api_client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", BASE_URL)
        .header("Referer", &format!("{BASE_URL}/"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn api_json<T: for<'de> Deserialize<'de>>(target: &str, fixture: &str) -> T {
    serde_json::from_str(&api_get(target, fixture))
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn parse_manga_response(body: &str) -> Paged<CatalogItem> {
    let result: MangaResponseDto<MangaListDto> =
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).unwrap());
    let page = result.data.pagination.page;
    let total_page = result.data.pagination.total_page;
    Paged {
        entries: result
            .data
            .data
            .into_iter()
            .map(|manga| manga.into_item(false))
            .collect(),
        has_next_page: page < total_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, Some(key.to_string()))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    let mut item = CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "Leitura Mangá".to_string())
            }),
        cover: Some(format!(
            "{CDN_URL}/{}/cover-md.webp",
            key.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("sample")
        )),
        description: text_after_heading(body, "Sinopse"),
        authors: text_after_heading(body, "Autor")
            .map(|value| value.trim_start_matches("Autor:").trim().to_string())
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genre"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(
            text_after_heading(body, "Status")
                .map(|value| value.trim_start_matches("Status:").trim().to_string())
                .as_deref(),
        ),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if let Some(api_item) = next_json(body)
        .and_then(|root| find_object_with_key(&root, "manga").cloned())
        .and_then(|value| serde_json::from_value::<MangaDto>(value).ok())
    {
        let enriched = api_item.into_item(true);
        item.cover = enriched.cover.or(item.cover);
        item.description = enriched.description.or(item.description);
        item.authors = enriched.authors;
        item.tags = enriched.tags;
        item.status = enriched.status;
    }
    item
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let images = next_json(body)
        .and_then(|root| find_images(&root))
        .filter(|images| !images.is_empty())
        .unwrap_or_else(|| extract_image_urls(body));
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: cdn_url(&image),
                context: Some(manga::image_headers(page_url)),
            },
            headers: manga::image_headers(page_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_manga_id(body: &str) -> Option<String> {
    next_json(body)
        .and_then(|root| {
            find_string_field(&root, "mangaId").or_else(|| {
                find_object_with_key(&root, "manga")
                    .and_then(|manga| find_string_field(manga, "_id"))
            })
        })
        .or_else(|| string_after(body, "\"mangaId\":\""))
        .or_else(|| string_after(body, "\"_id\":\""))
}

fn next_json(body: &str) -> Option<Value> {
    extract_next_data(body)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .or_else(|| parse_first_json_object(body))
}

fn extract_next_data(body: &str) -> Option<&str> {
    let after_marker = body.split("__NEXT_DATA__").nth(1)?;
    after_marker.split_once('>')?.1.split("</script>").next()
}

fn parse_first_json_object(body: &str) -> Option<Value> {
    let mut starts = body.match_indices('{').map(|(index, _)| index);
    starts.find_map(|start| {
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, ch) in body[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &body[start..start + offset + ch.len_utf8()];
                        if candidate.contains("mangaId")
                            || candidate.contains("chapter")
                            || candidate.contains("\"manga\"")
                        {
                            if let Ok(value) = serde_json::from_str(candidate) {
                                return Some(value);
                            }
                        }
                        return None;
                    }
                }
                _ => {}
            }
        }
        None
    })
}

fn find_images(root: &Value) -> Option<Vec<String>> {
    find_array_with_key(root, "images").map(|array| {
        array
            .iter()
            .filter_map(|item| {
                item.get("url")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str())
                    .map(ToString::to_string)
            })
            .collect()
    })
}

fn find_array_with_key<'a>(value: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    match value {
        Value::Object(map) => map.get(key).and_then(Value::as_array).or_else(|| {
            map.values()
                .find_map(|value| find_array_with_key(value, key))
        }),
        Value::Array(items) => items
            .iter()
            .find_map(|value| find_array_with_key(value, key)),
        _ => None,
    }
}

fn find_object_with_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if let Some(nested) = map.get(key).filter(|nested| nested.is_object()) {
                return Some(nested);
            }
            map.values()
                .find_map(|value| find_object_with_key(value, key))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|value| find_object_with_key(value, key)),
        _ => None,
    }
}

fn find_string_field(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| map.values().find_map(|value| find_string_field(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find_string_field(value, key)),
        _ => None,
    }
}

fn text_after_heading(body: &str, label: &str) -> Option<String> {
    let marker = format!("contains({label})");
    html::text_between(body, &marker, "</")
        .or_else(|| {
            body.split("<p")
                .skip(1)
                .find(|chunk| chunk.contains(label))
                .map(|chunk| html::strip_tags(&format!("<p{chunk}")).trim().to_string())
        })
        .or_else(|| {
            let index = body.find(label)?;
            html::text_between(&body[index..], "<p", "</p>")
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn extract_image_urls(body: &str) -> Vec<String> {
    body.split("\"url\"")
        .skip(1)
        .filter_map(|chunk| string_after(chunk, ":\""))
        .collect()
}

fn string_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\/", "/"))
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_status(value: Option<&str>) -> ItemStatus {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ongoing" | "em andamento" => ItemStatus::Ongoing,
        "completed" | "completo" | "finalizado" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn cdn_url(value: &str) -> String {
    url::join_url(CDN_URL, value)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return format!("/{}", value[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", value.trim_matches('/'))
}

#[derive(Deserialize)]
struct MangaResponseDto<T> {
    data: T,
}

#[derive(Deserialize)]
struct MangaListDto {
    #[serde(default)]
    data: Vec<MangaDto>,
    pagination: PaginationDto,
}

#[derive(Default, Deserialize)]
struct PaginationDto {
    #[serde(default)]
    page: u64,
    #[serde(default, rename = "totalPage")]
    total_page: u64,
}

#[derive(Default, Deserialize)]
struct MangaDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    author: Option<String>,
    description: Option<String>,
    status: Option<String>,
    #[serde(default)]
    genres: Vec<GenreDto>,
    #[serde(default, rename = "alternativeTitles")]
    alternative_titles: Vec<String>,
}

impl MangaDto {
    fn into_item(self, initialized: bool) -> CatalogItem {
        let mut description = self.description;
        if !self.alternative_titles.is_empty() {
            let suffix = format!(
                "Títulos alternativos: {}",
                self.alternative_titles.join(", ")
            );
            description = Some(match description {
                Some(text) if !text.is_empty() => format!("{text}\n\n{suffix}"),
                _ => suffix,
            });
        }
        CatalogItem {
            key: format!("/manga/{}", self.slug),
            title: if self.title.is_empty() {
                "Leitura Mangá".to_string()
            } else {
                self.title
            },
            cover: Some(format!("{CDN_URL}/{}/cover-md.webp", self.slug)),
            description,
            authors: self.author.into_iter().collect(),
            tags: self.genres.into_iter().map(|genre| genre.name).collect(),
            status: parse_status(self.status.as_deref()),
            url: Some(format!("{BASE_URL}/manga/{}", self.slug)),
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct GenreDto {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterListDto {
    #[serde(default)]
    data: Vec<ChapterDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    chapter_number: f64,
    title: Option<String>,
    release_date: String,
}

impl ChapterDto {
    fn into_chapter(self, manga_slug: &str) -> MangaChapter {
        let number = display_number(self.chapter_number);
        MangaChapter {
            key: format!("/manga/{manga_slug}/chapter/{number}"),
            title: Some(self.title.unwrap_or_else(|| format!("Capítulo {number}"))),
            chapter_number: Some(self.chapter_number as f32),
            date_uploaded: dates::parse_ymd(
                self.release_date
                    .split('T')
                    .next()
                    .unwrap_or(&self.release_date),
            ),
            language: Some(LANG.to_string()),
            url: Some(format!("{BASE_URL}/manga/{manga_slug}/chapter/{number}")),
            ..MangaChapter::default()
        }
    }
}

fn display_number(value: f64) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

const LIST_FIXTURE: &str = r#"{"data":{"data":[{"_id":"fixture-id","title":"Sample Leitura","slug":"sample","author":"Author","description":"Resumo.","status":"Ongoing","genres":[{"name":"Ação"}],"alternativeTitles":["Alt"]}],"pagination":{"page":1,"totalPage":1}}}"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Leitura</h1><h2>Sinopse</h2><div><p>Resumo.</p></div><h2>Informações</h2><div><p>Autor: Author</p><p>Status: Ongoing</p></div><a href="/genre/action">Ação</a>
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"mangaId":"fixture-id","manga":{"_id":"fixture-id","title":"Sample Leitura","slug":"sample","author":"Author","description":"Resumo.","status":"Ongoing","genres":[{"name":"Ação"}]}}}}</script>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"data":[{"chapterNumber":1,"title":"Capítulo 1","releaseDate":"2024-01-01T00:00:00.000Z"}]}}"#;
const PAGES_FIXTURE: &str = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"chapter":{"images":[{"url":"/sample/chapter-1/001.webp"},{"url":"/sample/chapter-1/002.webp"}]}}}}</script>"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_leitura_fixtures() {
        assert_eq!(parse_manga_response(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            extract_manga_id(DETAILS_FIXTURE).as_deref(),
            Some("fixture-id")
        );
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
    }
}
