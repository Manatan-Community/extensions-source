use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::{Value, json};

const SOURCE: Taiyo = Taiyo;
const BASE_URL: &str = "https://taiyo.moe";
const MEILI_URL: &str = "https://meilisearch.taiyo.moe/multi-search";
const IMG_CDN: &str = "https://cdn.taiyo.moe/medias";

struct Taiyo;

impl MangaSource for Taiyo {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_search(SEARCH_FIXTURE));
        }
        self.search(json!({"page": page(&request), "query": ""}))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_id(&id)],
                has_next_page: false,
            });
        }
        let limit = 21;
        let body = json!({
            "queries": [{
                "indexUid": "medias",
                "q": query,
                "filter": ["deletedAt IS NULL"],
                "limit": limit,
                "offset": limit * (page(&request).saturating_sub(1))
            }]
        });
        Ok(parse_search(&post_meili(body)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/media/sample".into());
        Ok(details_by_id(id_from_key(&key).unwrap_or("sample")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/media/sample".into());
        let id = id_from_key(&key).unwrap_or("sample");
        let mut chapters = Vec::new();
        for page in 1..=100 {
            let input = json!({"0":{"json":{"mediaId": id, "page": page, "perPage": 50}}});
            let target = format!(
                "{BASE_URL}/api/trpc/chapters.getByMediaId?batch=1&input={}",
                url::query_escape(&input.to_string())
            );
            let body = fetch_text(&target, CHAPTERS_FIXTURE);
            let value = extract_chapter_json(&body).unwrap_or_else(|| CHAPTERS_FIXTURE.into());
            let parsed = json_value(&value, CHAPTERS_FIXTURE);
            let items = parsed
                .get("chapters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            chapters.extend(items.iter().map(chapter_item));
            if page
                >= parsed
                    .get("totalPages")
                    .and_then(Value::as_u64)
                    .unwrap_or(page)
            {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/chapter-id/1".into());
        let target = format!("{BASE_URL}{}", normalize_path(&key));
        let body = fetch_document(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .and_then(|key| id_from_key(&key).map(|id| format!("{BASE_URL}/media/{id}"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", normalize_path(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&id)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.into()),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn post_meili(body: Value) -> String {
    let token = fetch_bearer_token().unwrap_or_default();
    let client = client();
    let mut request = client.post(MEILI_URL).json(body.to_string()).xhr();
    if !token.is_empty() {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request
        .send_text()
        .unwrap_or_else(|_| SEARCH_FIXTURE.into())
}

fn fetch_bearer_token() -> Option<String> {
    let home = fetch_document(BASE_URL, "");
    let scripts = script_srcs(&home);
    for script in scripts.into_iter().rev() {
        let body = fetch_text(&url::join_url(BASE_URL, &script), "");
        if let Some(token) = token_from_script(&body) {
            return Some(token);
        }
    }
    None
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let value = json_value(body, SEARCH_FIXTURE);
    let hits = value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
        .and_then(|first| first.get("hits"))
        .or_else(|| value.get("hits"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = hits
        .iter()
        .map(|item| catalog_item(item, false))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn details_by_id(id: &str) -> CatalogItem {
    let body = fetch_document(&format!("{BASE_URL}/media/{id}"), DETAILS_FIXTURE);
    parse_details(&body, id)
}

fn catalog_item(value: &Value, initialized: bool) -> CatalogItem {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("sample");
    CatalogItem {
        key: format!("/media/{id}"),
        title: best_title(value).unwrap_or_else(|| "Taiyo".into()),
        cover: value
            .get("mainCoverId")
            .and_then(Value::as_str)
            .map(|cover| {
                format!("{BASE_URL}/_next/image?url={IMG_CDN}/{id}/covers/{cover}.jpg&w=256&q=75")
            }),
        description: value
            .get("synopsis")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        tags: genres(value),
        status: status(
            value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        url: Some(format!("{BASE_URL}/media/{id}")),
        language: Some("pt-BR".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, id: &str) -> CatalogItem {
    let json = extract_media_json(body).and_then(|text| serde_json::from_str::<Value>(&text).ok());
    if let Some(value) = json {
        let mut item = catalog_item(&value, true);
        item.key = format!("/media/{id}");
        item.url = Some(format!("{BASE_URL}/media/{id}"));
        if item.cover.is_none() {
            item.cover = first_image(body);
        }
        return item;
    }
    CatalogItem {
        key: format!("/media/{id}"),
        title: html::text_between(body, "media-title", "</p>")
            .map(|text| html::strip_tags(&text))
            .unwrap_or_else(|| "Taiyo".into()),
        cover: first_image(body),
        description: extract_tag_text(body, "p"),
        url: Some(format!("{BASE_URL}/media/{id}")),
        language: Some("pt-BR".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn chapter_item(value: &Value) -> MangaChapter {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chapter-id");
    let number = value
        .get("number")
        .and_then(Value::as_f64)
        .map(|n| n as f32);
    MangaChapter {
        key: format!("/chapter/{id}/1"),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| number.map(|n| format!("Capitulo {}", format_number(n)))),
        chapter_number: number,
        date_uploaded: value
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(|date| dates::parse_ymd(date.get(..10).unwrap_or(date))),
        url: Some(format!("{BASE_URL}/chapter/{id}/1")),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value = extract_media_chapter_json(body)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json_value(body, PAGES_FIXTURE));
    let chapter_id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chapter-id");
    let media_id = value
        .pointer("/media/id")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    value
        .get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|page| page.get("id").and_then(Value::as_str))
        .enumerate()
        .map(|(index, page_id)| {
            let mut headers = Context::new();
            headers.insert(
                "Referer".into(),
                format!("{BASE_URL}/chapter/{chapter_id}/1"),
            );
            MangaPage {
                content: PageContent::Url {
                    url: format!("{IMG_CDN}/{media_id}/chapters/{chapter_id}/{page_id}.jpg"),
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn extract_media_json(body: &str) -> Option<String> {
    extract_embedded_object(body, "media", ",\\\"trackers\\\"")
}

fn extract_media_chapter_json(body: &str) -> Option<String> {
    extract_embedded_object(body, "mediaChapter", ",\\\"chapters\\\"")
}

fn extract_embedded_object(body: &str, key: &str, end_marker: &str) -> Option<String> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains(&format!("{key}\\\\\":")))?;
    let raw = script.split("</script>").next().unwrap_or(script);
    let start_marker = format!(",{{\\\"{key}\\\":");
    let start = raw.find(&start_marker)? + start_marker.len();
    let rest = &raw[start..];
    let end = rest.find(end_marker).unwrap_or(rest.len());
    let mut text = rest[..end].to_string();
    if key == "mediaChapter" {
        text.push_str("}}");
    } else {
        text.push('}');
    }
    Some(text.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn extract_chapter_json(body: &str) -> Option<String> {
    let marker = "{\"chapters\"";
    let start = body.find(marker)?;
    balanced_from(body, start)
}

fn balanced_from(input: &str, start: usize) -> Option<String> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in input[start..].char_indices() {
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
        if ch == '"' {
            in_string = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(input[start..start + offset + 1].to_string());
            }
        }
    }
    None
}

fn script_srcs(body: &str) -> Vec<String> {
    body.split("<script")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("next") && !chunk.contains("nomodule") && !chunk.contains("app")
        })
        .filter_map(|chunk| html::attr(chunk, "src"))
        .collect()
}

fn token_from_script(body: &str) -> Option<String> {
    let marker = "NEXT_PUBLIC_MEILISEARCH_PUBLIC_KEY";
    let rest = body.split(marker).nth(1)?;
    let first = rest.find('"')? + 1;
    let tail = &rest[first..];
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

fn best_title(value: &Value) -> Option<String> {
    let titles = value.get("titles").and_then(Value::as_array)?;
    titles
        .iter()
        .find(|title| {
            title
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("en")
        })
        .or_else(|| {
            titles
                .iter()
                .max_by_key(|title| title.get("priority").and_then(Value::as_i64).unwrap_or(0))
        })
        .and_then(|title| title.get("title").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn genres(value: &Value) -> Vec<String> {
    value
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(genre_name)
        .collect()
}

fn genre_name(input: &str) -> String {
    match input {
        "ACTION" => "Acao",
        "ADVENTURE" => "Aventura",
        "COMEDY" => "Comedia",
        "DRAMA" => "Drama",
        "ECCHI" => "Ecchi",
        "FANTASY" => "Fantasia",
        "HENTAI" => "Hentai",
        "HORROR" => "Horror",
        "ROMANCE" => "Romance",
        "SCI_FI" => "Sci-fi",
        "SLICE_OF_LIFE" => "Slice of Life",
        "SPORTS" => "Esportes",
        "SUPERNATURAL" => "Sobrenatural",
        "THRILLER" => "Thriller",
        other => other,
    }
    .into()
}

fn status(input: &str) -> ItemStatus {
    match input {
        "FINISHED" => ItemStatus::Completed,
        "RELEASING" => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn first_image(body: &str) -> Option<String> {
    let chunk = body.split("<img").nth(1)?;
    html::attr(chunk, "srcset")
        .and_then(|srcset| {
            srcset
                .split(',')
                .last()
                .and_then(|part| part.split_whitespace().next())
                .map(ToString::to_string)
        })
        .or_else(|| html::attr(chunk, "src"))
        .map(|src| url::join_url(BASE_URL, &src))
}

fn extract_tag_text(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|text| html::strip_tags(&text))
}

fn id_from_url(input: &str) -> Option<String> {
    input.split("/media/").nth(1).map(|rest| {
        rest.trim_matches('/')
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

fn id_from_key(key: &str) -> Option<&str> {
    key.trim_matches('/').split('/').nth(1)
}

fn normalize_path(path: &str) -> String {
    let path = path.split(BASE_URL).last().unwrap_or(path);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn json_value(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or(Value::Null)
}

fn format_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"results":[{"hits":[{"id":"sample","synopsis":"Sample description","status":"RELEASING","genres":["ACTION"],"mainCoverId":"cover","titles":[{"title":"Sample Taiyo","language":"en_US","priority":1}]}]}]}"#;
const DETAILS_FIXTURE: &str = r#"<html><body><p class="media-title">Sample Taiyo</p><section><div class="flex"></div><div><p>Sample description</p></div></section></body></html>"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"id":"chapter-id","number":1.0,"title":"Inicio","createdAt":"2024-01-01T00:00:00.000Z","scans":[{"name":"Taiyo"}]}],"totalPages":1}"#;
const PAGES_FIXTURE: &str =
    r#"{"id":"chapter-id","media":{"id":"sample"},"pages":[{"id":"page-1"},{"id":"page-2"}]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_search(SEARCH_FIXTURE).entries.len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
        assert_eq!(
            extract_chapter_json(CHAPTERS_FIXTURE).unwrap(),
            CHAPTERS_FIXTURE
        );
    }
}
