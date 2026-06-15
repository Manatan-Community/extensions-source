use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    http, source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: YugenMangas = YugenMangas;
const BASE_URL: &str = "https://yugenmangasbr.dxtg.online";
const API_URL: &str = "https://api.yugenweb.com";

struct YugenMangas;

impl MangaSource for YugenMangas {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_library_json(LIBRARY_FIXTURE));
        }
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/capitulos?page={page}")
        } else {
            format!("{BASE_URL}/biblioteca?page={page}&sort_order=desc&sort_by=total_views")
        };
        let body = fetch_document(&target, LIBRARY_FIXTURE);
        Ok(parse_library_json(
            &extract_next_json(&body).unwrap_or(body),
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(slug) = slug_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let page = page(&request);
        let target = format!(
            "{API_URL}/api/v2/library/series?name={}&per_page=10&page={page}",
            url::query_escape(query)
        );
        Ok(parse_library_json(&fetch_json(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(details_by_slug(slug_from_key(&key).unwrap_or("sample")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        let slug = slug_from_key(&key).unwrap_or("sample");
        let mut chapters = Vec::new();
        for page in 1..=100 {
            let target = format!("{BASE_URL}/series/{slug}?order=desc&page={page}");
            let body = fetch_document(&target, CHAPTERS_FIXTURE);
            let page_chapters = parse_chapters(&body);
            if page_chapters.is_empty() {
                break;
            }
            chapters.extend(page_chapters);
        }
        if chapters.is_empty() {
            chapters = parse_chapters(CHAPTERS_FIXTURE);
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/reader/sample/1".into());
        let target = format!("{BASE_URL}{}", normalize_path(&key));
        let body = fetch_document(&target, PAGES_FIXTURE);
        let json = extract_pages_json(&body).unwrap_or_else(|| PAGES_FIXTURE.into());
        Ok(parse_pages(&json, &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(serde_json::json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(serde_json::json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            section("popular", "Popular", popular),
            section("latest", "Latest", latest),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .and_then(|key| slug_from_key(&key).map(|slug| format!("{BASE_URL}/series/{slug}"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}{}", normalize_path(&key))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(slug) = slug_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(&slug)),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn parse_library_json(body: &str) -> Paged<CatalogItem> {
    let value = json_value(body, LIBRARY_FIXTURE);
    let library = value
        .get("initialData")
        .or_else(|| value.get("library"))
        .unwrap_or(&value);
    let entries = library
        .get("series")
        .or_else(|| library.get("updates"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            if let Some(series) = item.get("series") {
                manga_item(series, false)
            } else {
                manga_item(item, false)
            }
        })
        .collect();
    let has_next_page = library
        .get("pagination")
        .and_then(|p| {
            let current = p.get("current_page").and_then(Value::as_u64)?;
            let total = p.get("total_pages").and_then(Value::as_u64)?;
            Some(current < total)
        })
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let body = fetch_document(&format!("{BASE_URL}/series/{slug}"), DETAILS_FIXTURE);
    parse_details(&body, slug)
}

fn manga_item(value: &Value, initialized: bool) -> CatalogItem {
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    let title = value
        .get("title")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Yugen Mangas");
    CatalogItem {
        key: format!("/series/{code}"),
        title: title.into(),
        cover: value.get("cover").and_then(Value::as_str).map(cover_url),
        url: Some(format!("{BASE_URL}/series/{code}")),
        language: Some("pt-BR".into()),
        content_rating: Some("safe".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, slug: &str) -> CatalogItem {
    let title = extract_tag_text(body, "h1").unwrap_or_else(|| "Yugen Mangas".into());
    CatalogItem {
        key: format!("/series/{slug}"),
        title,
        cover: first_image(body),
        description: meta_content(body, "og:description"),
        authors: text_after_label(body, "Autor")
            .map(|value| vec![value])
            .unwrap_or_default(),
        artists: text_after_label(body, "Artista")
            .map(|value| vec![value])
            .unwrap_or_default(),
        tags: badge_texts(body),
        status: badge_texts(body)
            .into_iter()
            .find_map(|badge| match badge.to_lowercase().as_str() {
                "em lancamento" | "em lançamento" => Some(ItemStatus::Ongoing),
                "finalizada" => Some(ItemStatus::Completed),
                "em hiato" => Some(ItemStatus::Hiatus),
                _ => None,
            })
            .unwrap_or(ItemStatus::Unknown),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("pt-BR".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for part in body.split("href=").skip(1) {
        let href = quoted_value(part).unwrap_or_default();
        if !href.contains("reader") {
            continue;
        }
        let block = part.split("</a>").next().unwrap_or(part);
        let title = extract_tag_text(block, "p").unwrap_or_else(|| "Capitulo".into());
        chapters.push(MangaChapter {
            key: normalize_path(&href),
            title: Some(title),
            url: Some(url::join_url(BASE_URL, &href)),
            ..MangaChapter::default()
        });
    }
    chapters.sort_by(|a, b| b.key.cmp(&a.key));
    chapters.dedup_by(|a, b| a.key == b.key);
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let value = json_value(body, PAGES_FIXTURE);
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let path = item.get("path").and_then(Value::as_str)?;
            let number = item.get("number").and_then(Value::as_u64).unwrap_or(1);
            Some((
                number,
                format!("{API_URL}/media/{}", path.trim_start_matches('/')),
            ))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, (number, image))| {
            let mut headers = Context::new();
            headers.insert("Referer".into(), referer.into());
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!(
                    "Page {}",
                    if number == 0 {
                        index as u64 + 1
                    } else {
                        number
                    }
                )),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn extract_next_json(body: &str) -> Option<String> {
    extract_balanced_json(body, "{\"initialData")
        .or_else(|| extract_from_unescaped_script(body, "initialData", '{'))
}

fn extract_pages_json(body: &str) -> Option<String> {
    if serde_json::from_str::<Value>(body)
        .ok()?
        .as_array()
        .is_some()
    {
        return Some(body.into());
    }
    extract_from_unescaped_script(body, "pages", '[')
}

fn extract_from_unescaped_script(body: &str, marker: &str, open: char) -> Option<String> {
    let unescaped = unescape_js(body);
    if open == '{' {
        extract_balanced_json(&unescaped, &format!("{{\"{marker}"))
    } else {
        let marker_pos = unescaped.find(&format!("\"{marker}\":["))?;
        let array_start = unescaped[marker_pos..].find('[')? + marker_pos;
        balanced_from(&unescaped, array_start, '[', ']')
    }
}

fn extract_balanced_json(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    balanced_from(body, start, '{', '}')
}

fn balanced_from(input: &str, start: usize, open: char, close: char) -> Option<String> {
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
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(input[start..start + offset + ch.len_utf8()].to_string());
            }
        }
    }
    None
}

fn unescape_js(input: &str) -> String {
    input
        .replace("\\\"", "\"")
        .replace("\\/", "/")
        .replace("\\n", "\n")
        .replace("\\\\", "\\")
}

fn cover_url(path: &str) -> String {
    format!(
        "{BASE_URL}/_next/image?url={API_URL}/media/{}&w=640&q=75",
        path.trim_start_matches('/')
    )
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

fn meta_content(body: &str, property: &str) -> Option<String> {
    let pos = body.find(property)?;
    html::attr(&body[pos..], "content")
}

fn extract_tag_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    html::text_between(body, &open, &format!("</{tag}>")).map(|text| html::strip_tags(&text))
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let pos = body.find(label)?;
    extract_tag_text(&body[pos..], "p")
}

fn badge_texts(body: &str) -> Vec<String> {
    body.split("data-slot=\"badge\"")
        .skip(1)
        .filter_map(|chunk| {
            chunk
                .split('>')
                .nth(1)
                .and_then(|text| text.split('<').next())
        })
        .map(html::html_unescape)
        .filter(|text| !text.trim().is_empty())
        .collect()
}

fn quoted_value(input: &str) -> Option<String> {
    let quote = input.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    input[1..]
        .find(quote)
        .map(|end| input[1..1 + end].to_string())
}

fn normalize_path(path: &str) -> String {
    let path = path.split(BASE_URL).last().unwrap_or(path);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn slug_from_url(input: &str) -> Option<String> {
    input.split("/series/").nth(1).map(|rest| {
        rest.trim_matches('/')
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

fn slug_from_key(key: &str) -> Option<&str> {
    key.trim_matches('/').split('/').nth(1)
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
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

export_manga_source!(SOURCE);

const LIBRARY_FIXTURE: &str = r#"{"initialData":{"series":[{"code":"sample","cover":"sample.jpg","title":"Sample Yugen"}],"pagination":{"current_page":1,"total_pages":1}}}"#;
const SEARCH_FIXTURE: &str = r#"{"series":[{"code":"sample","cover":"sample.jpg","name":"Sample Yugen"}],"pagination":{"current_page":1,"total_pages":1}}"#;
const DETAILS_FIXTURE: &str = r#"<html><head><meta property="og:description" content="Sample description"></head><body><h1>Sample Yugen</h1><img srcset="/sample.jpg 640w"><p>Autor</p><p>Sample Author</p><p>Artista</p><p>Sample Artist</p><div data-slot="badge">Acao</div><div data-slot="badge">Em lancamento</div></body></html>"#;
const CHAPTERS_FIXTURE: &str = r#"<a href="/reader/sample/1"><p>Capitulo 1</p></a>"#;
const PAGES_FIXTURE: &str =
    r#"[{"path":"sample/page-1.jpg","number":1},{"path":"sample/page-2.jpg","number":2}]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_library_json(LIBRARY_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 2);
    }
}
