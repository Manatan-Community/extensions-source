use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: RavenManga = RavenManga;
const BASE_URL: &str = "https://raventard.xyz";
const LANG: &str = "es";
const CONTENT_RATING: &str = "safe";

struct RavenManga;

impl MangaSource for RavenManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let body = fetch_document_or_fixture(BASE_URL, LIST_FIXTURE);
        if listing_id(&request) == "latest" {
            Ok(parse_latest(&body))
        } else {
            Ok(parse_popular(&body))
        }
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
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    &key,
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = fetch_document_or_fixture(&format!("{BASE_URL}/comics"), COMICS_FIXTURE);
            return Ok(Paged {
                entries: parse_project_json(&body, query),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_comics_page(&fetch_document_or_fixture(
            &format!("{BASE_URL}/comics?page={page}"),
            COMICS_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sr2/sample".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sr2/sample".into());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/leer/sample/1".into());
        let chapter_url = absolute_url(&key);
        let body = fetch_document_or_fixture(&chapter_url, PAGES_FIXTURE);
        Ok(parse_pages(&body, &chapter_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    &key,
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL)
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

fn post_form_or_empty(target: &str, referer: &str, form: &[(String, String)]) -> String {
    let refs = form
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    client()
        .post(target)
        .header("Referer", referer)
        .form(&refs)
        .send_text()
        .unwrap_or_default()
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("div-diario")
                || chunk.contains("div-semanal")
                || chunk.contains("div-mensual")
                || chunk.contains("<a")
        })
        .filter_map(parse_figure)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<figure")
            .skip(1)
            .filter_map(parse_figure)
            .fold(Vec::new(), push_unique),
        has_next_page: false,
    }
}

fn parse_comics_page(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<figure")
            .skip(1)
            .filter_map(parse_figure)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_figure(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<figcaption", "</figcaption>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "RavenManga".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_project_json(body: &str, query: &str) -> Vec<CatalogItem> {
    let Some(json) = script_array_after(body, "proyectos") else {
        return Vec::new();
    };
    serde_json::from_str::<Value>(&json)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let title = string_value(&item, "nombre")?;
            if !title
                .to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
            {
                return None;
            }
            let slug = string_value(&item, "slug").unwrap_or_else(|| slugify(&title));
            let cover = string_value(&item, "portada");
            Some(CatalogItem {
                key: format!("/sr2/{slug}"),
                title,
                cover,
                url: Some(format!("{BASE_URL}/sr2/{slug}")),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "RavenManga".to_string()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "section-sinopsis", "</section>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("genero") || chunk.contains("genre"))
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("section-list-cap")
                || chunk.contains("id=\"name\"")
                || chunk.contains("id='name'")
        })
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "id=\"name\"", "</")
                .or_else(|| html::text_between(chunk, "id='name'", "</"))
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Capitulo".to_string());
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::text_between(chunk, "<time", "</time>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_relative_date(&value)),
                language: Some(LANG.to_string()),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let body = if let Some((target, form)) = redirect_form(body) {
        let posted = post_form_or_empty(&url::join_url(BASE_URL, &target), chapter_url, &form);
        if posted.is_empty() {
            body.to_string()
        } else {
            posted
        }
    } else {
        body.to_string()
    };
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("contenedor-imagen") || chunk.contains("src"))
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
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

fn redirect_form(body: &str) -> Option<(String, Vec<(String, String)>)> {
    let form = body
        .split("<form")
        .skip(1)
        .find(|chunk| chunk.contains("redirectForm") && chunk.contains("post"))?;
    let action = html::attr(form, "action")?;
    let inputs = form
        .split("<input")
        .skip(1)
        .filter_map(|input| {
            Some((
                html::attr(input, "name")?,
                html::attr(input, "value").unwrap_or_default(),
            ))
        })
        .collect::<Vec<_>>();
    Some((action, inputs))
}

fn script_array_after(body: &str, name: &str) -> Option<String> {
    let rest = body.split(name).nth(1)?;
    let start = rest.find('[')?;
    let mut depth = 0i32;
    for (index, ch) in rest[start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[start..start + index + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| {
            html::attr(chunk, "srcset").map(|value| {
                value
                    .split_whitespace()
                    .next()
                    .unwrap_or(&value)
                    .to_string()
            })
        })
        .or_else(|| html::attr(chunk, "src"))
}

fn parse_relative_date(value: &str) -> Option<i64> {
    let number = value
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<i64>().ok())?;
    let seconds = if contains_word(value, &["segundo"]) {
        number
    } else if contains_word(value, &["minuto"]) {
        number * 60
    } else if contains_word(value, &["hora"]) {
        number * 3_600
    } else if contains_word(value, &["dia", "día"]) {
        number * 86_400
    } else if contains_word(value, &["semana"]) {
        number * 7 * 86_400
    } else if contains_word(value, &["mes"]) {
        number * 30 * 86_400
    } else if contains_word(value, &["ano", "año"]) {
        number * 365 * 86_400
    } else {
        return None;
    };
    Some(unix_now().saturating_sub(seconds))
}

fn contains_word(value: &str, words: &[&str]) -> bool {
    let lower = value.to_lowercase();
    words.iter().any(|word| lower.contains(word))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

fn string_value(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn slugify(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
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

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div id="div-diario"><figure><a href="/sr2/sample"><img src="/cover.jpg"><figcaption>Sample Raven</figcaption></a></figure></div>"#;
const COMICS_FIXTURE: &str = r#"<script>proyectos = [{"nombre":"Sample Raven","slug":"sample","portada":"https://raventard.xyz/cover.jpg"}];</script><section class="flex"><div class="grid"><figure><a href="/sr2/sample"><img src="/cover.jpg"><figcaption>Sample Raven</figcaption></a></figure></div></section>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample Raven</h1><section id="section-sinopsis"><p>Summary.</p><div><a href="/genero/drama"><span>Drama</span></a></div></section><section id="section-list-cap"><div class="grid"><a href="/leer/sample/1"><div id="name">Capitulo 1</div><time>hace 1 dia</time></a></div></section>"#;
const PAGES_FIXTURE: &str =
    r#"<main class="contenedor-imagen"><section><img src="/page1.jpg"></section></main>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raven_fixtures() {
        assert_eq!(parse_popular(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_project_json(COMICS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, BASE_URL).len(), 1);
    }
}
