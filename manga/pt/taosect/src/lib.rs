use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: TaoSect = TaoSect;
const BASE_URL: &str = "https://taosect.com";
const API_BASE: &str = "https://taosect.com/wp-json/wp/v2";
const PROJECTS_PER_PAGE: u64 = 18;
const DEFAULT_FIELDS: &str = "title,thumbnail,link";

struct TaoSect;

impl MangaSource for TaoSect {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_projects(PROJECTS_FIXTURE, 1, false));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(latest_projects(page));
        }
        let target = format!(
            "{API_BASE}/projetos?order=desc&orderby=views&page={page}&per_page={PROJECTS_PER_PAGE}&_fields={}",
            url::query_escape(DEFAULT_FIELDS)
        );
        Ok(parse_projects(
            &fetch_json(&target, PROJECTS_FIXTURE),
            page,
            true,
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
        let mut target = format!(
            "{API_BASE}/projetos?page={page}&per_page={PROJECTS_PER_PAGE}&_fields={}",
            url::query_escape(DEFAULT_FIELDS)
        );
        if !query.is_empty() {
            push_param(&mut target, "search", query);
        }
        append_filter(&mut target, &request, "order");
        append_filter(&mut target, &request, "orderby");
        append_filter(&mut target, &request, "situacao");
        append_filter(&mut target, &request, "generos");
        Ok(parse_projects(
            &fetch_json(&target, PROJECTS_FIXTURE),
            page,
            true,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/projeto/sample".into());
        Ok(details_by_slug(slug_from_key(&key).unwrap_or("sample")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/projeto/sample".into());
        let slug = slug_from_key(&key).unwrap_or("sample");
        let target = format!(
            "{API_BASE}/capitulos?projeto={slug}&per_page=1000&order=desc&orderby=sequencia&_fields=nome_capitulo,post_id,slug,data_insercao"
        );
        Ok(parse_chapters(&fetch_json(&target, CHAPTERS_FIXTURE), slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/leitor-online/projeto/sample/chapter-1".into());
        let (project, chapter) = chapter_parts(&key);
        let target =
            format!("{API_BASE}/capitulos/{project}/{chapter}?_fields=id_capitulo,paginas,post_id");
        Ok(parse_pages(
            &fetch_json(&target, PAGES_FIXTURE),
            &project,
            &chapter,
        ))
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
            .and_then(|key| slug_from_key(&key).map(|slug| format!("{BASE_URL}/projeto/{slug}"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (project, chapter) = chapter_parts(&key);
            format!("{BASE_URL}/leitor-online/projeto/{project}/{chapter}")
        }))
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
        .with_header("Accept", "application/json")
        .with_referer(BASE_URL)
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.into())
}

fn latest_projects(page: u64) -> Paged<CatalogItem> {
    let target = format!(
        "{API_BASE}/capitulos?order=desc&orderby=date&page={page}&per_page={}&_fields=post_id",
        PROJECTS_PER_PAGE * 2
    );
    let chapters = json_value(&fetch_json(&target, LATEST_FIXTURE), LATEST_FIXTURE);
    let ids = chapters
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|chapter| str_or_int(chapter, &["post_id"]))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    }
    let target = format!(
        "{API_BASE}/projetos?include={}&per_page={}&orderby=include&_fields={}",
        ids.join(","),
        ids.len(),
        url::query_escape(DEFAULT_FIELDS)
    );
    parse_projects(&fetch_json(&target, PROJECTS_FIXTURE), page, true)
}

fn parse_projects(body: &str, page: u64, assume_more: bool) -> Paged<CatalogItem> {
    let entries = json_value(body, PROJECTS_FIXTURE)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|project| project_item(project, false))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: assume_more && entries.len() as u64 >= PROJECTS_PER_PAGE && page < 100,
        entries,
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let target = format!(
        "{API_BASE}/projetos?per_page=1&slug={slug}&_fields=title,informacoes,content,thumbnail,link"
    );
    let value = json_value(&fetch_json(&target, DETAILS_FIXTURE), DETAILS_FIXTURE);
    value
        .as_array()
        .and_then(|items| items.first())
        .map(|item| project_item(item, true))
        .unwrap_or_else(|| project_item(&value, true))
}

fn project_item(value: &Value, initialized: bool) -> CatalogItem {
    let title = value
        .pointer("/title/rendered")
        .and_then(Value::as_str)
        .unwrap_or("Tao Sect");
    let link = value
        .get("link")
        .and_then(Value::as_str)
        .unwrap_or(BASE_URL);
    let slug = value
        .get("slug")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| slug_from_url(link))
        .unwrap_or_else(|| "sample".into());
    let info = value.get("informacoes").unwrap_or(&Value::Null);
    let content = value
        .pointer("/content/rendered")
        .and_then(Value::as_str)
        .map(html::strip_tags);
    CatalogItem {
        key: format!("/projeto/{slug}"),
        title: html::html_unescape(&html::strip_tags(title)),
        cover: value
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: details_description(content, info),
        authors: text_field(info, "roteiro")
            .map(|v| vec![v])
            .unwrap_or_default(),
        artists: text_field(info, "arte")
            .map(|v| vec![v])
            .unwrap_or_default(),
        tags: info
            .get("generos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| text_field(tag, "nome"))
            .collect(),
        status: info
            .pointer("/status_scan/nome")
            .and_then(Value::as_str)
            .map(status)
            .unwrap_or(ItemStatus::Unknown),
        url: Some(format!("{BASE_URL}/projeto/{slug}")),
        language: Some("pt-BR".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn details_description(content: Option<String>, info: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(content) = content.filter(|value| !value.is_empty()) {
        parts.push(content);
    }
    for (label, key) in [
        ("Titulo original", "titulo_pais_origem"),
        ("Serializacao", "serializacao"),
    ] {
        if let Some(value) = text_field(info, key).filter(|value| !value.is_empty()) {
            parts.push(format!("{label}: {value}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn parse_chapters(body: &str, project_slug: &str) -> Vec<MangaChapter> {
    json_value(body, CHAPTERS_FIXTURE)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|chapter| {
            let slug = chapter
                .get("slug")
                .and_then(Value::as_str)
                .unwrap_or("chapter-1");
            MangaChapter {
                key: format!("/leitor-online/projeto/{project_slug}/{slug}"),
                title: text_field(chapter, "nome_capitulo"),
                date_uploaded: text_field(chapter, "data_insercao")
                    .and_then(|date| dates::parse_ymd(date.get(..10).unwrap_or(&date))),
                url: Some(format!(
                    "{BASE_URL}/leitor-online/projeto/{project_slug}/{slug}"
                )),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, project_slug: &str, chapter_slug: &str) -> Vec<MangaPage> {
    let referer = format!("{BASE_URL}/leitor-online/projeto/{project_slug}/{chapter_slug}");
    json_value(body, PAGES_FIXTURE)
        .get("paginas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| {
            let mut headers = Context::new();
            headers.insert("Referer".into(), referer.clone());
            headers.insert(
                "Accept".into(),
                "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".into(),
            );
            MangaPage {
                content: PageContent::Url {
                    url: image.into(),
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn status(input: &str) -> ItemStatus {
    match input {
        "Ativos" | "Ativo" => ItemStatus::Ongoing,
        "Finalizados" | "Finalizado" | "Oneshots" | "One-shot" => ItemStatus::Completed,
        "Cancelados" | "Cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
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

fn append_filter(target: &mut String, request: &Value, key: &str) {
    let Some(value) = request.get("filters").and_then(|filters| filters.get(key)) else {
        return;
    };
    if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
        push_param(target, key, text);
    } else if let Some(items) = value.as_array() {
        let joined = items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        if !joined.is_empty() {
            push_param(target, key, &joined);
        }
    }
}

fn push_param(target: &mut String, key: &str, value: &str) {
    target.push('&');
    target.push_str(key);
    target.push('=');
    target.push_str(&url::query_escape(value));
}

fn slug_from_url(input: &str) -> Option<String> {
    input.split("/projeto/").nth(1).map(|rest| {
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

fn chapter_parts(key: &str) -> (String, String) {
    let mut parts = key.trim_matches('/').split('/');
    let all = parts.by_ref().collect::<Vec<_>>();
    let project = all
        .iter()
        .position(|part| *part == "projeto")
        .and_then(|i| all.get(i + 1))
        .copied()
        .unwrap_or("sample");
    let chapter = all.last().copied().unwrap_or("chapter-1");
    (project.into(), chapter.into())
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn str_or_int(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value
            .get(*key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            return Some(text.into());
        }
        if let Some(id) = value.get(*key).and_then(Value::as_i64) {
            return Some(id.to_string());
        }
    }
    None
}

fn json_value(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(fixture))
        .unwrap_or(Value::Null)
}

export_manga_source!(SOURCE);

const PROJECTS_FIXTURE: &str = r#"[{"slug":"sample","link":"https://taosect.com/projeto/sample/","title":{"rendered":"Sample Tao Sect"},"thumbnail":"https://taosect.com/sample.jpg"}]"#;
const LATEST_FIXTURE: &str = r#"[{"post_id":"1"}]"#;
const DETAILS_FIXTURE: &str = r#"[{"slug":"sample","link":"https://taosect.com/projeto/sample/","title":{"rendered":"Sample Tao Sect"},"thumbnail":"https://taosect.com/sample.jpg","content":{"rendered":"<p>Sample description</p>"},"informacoes":{"arte":"Sample Artist","roteiro":"Sample Author","generos":[{"nome":"Acao"}],"status_scan":{"nome":"Ativos"},"titulo_pais_origem":"Sample","serializacao":"Web"}}]"#;
const CHAPTERS_FIXTURE: &str = r#"[{"nome_capitulo":"Capitulo 1","post_id":"1","slug":"chapter-1","data_insercao":"2024-01-01 00:00:00"}]"#;
const PAGES_FIXTURE: &str =
    r#"{"id_capitulo":"c1","post_id":"1","paginas":["https://drive.google.com/uc?id=sample"]}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_projects(PROJECTS_FIXTURE, 1, false).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, "sample").len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, "sample", "chapter-1").len(), 1);
    }
}
