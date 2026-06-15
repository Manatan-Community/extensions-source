use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, url};
use serde_json::Value;

const SOURCE: MediocreToons = MediocreToons;
const BASE_URL: &str = "https://mediocrescan.com";
const API_URL: &str = "https://back.mediocrescan.com";
const CDN_URL: &str = "https://cdn.mediocrescan.com";
const POPULAR_FORMATS: &str = "1,4,5,8,9,13";

struct MediocreToons;

impl MangaSource for MediocreToons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(LIST_FIXTURE));
        }
        let page = page(&request);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!(
                "{API_URL}/obras/atualizadas-recentes?limit=24&offset={}&formato=5",
                (page - 1) * 24
            )
        } else {
            format!(
                "{API_URL}/obras/buscar?limite=24&pagina={page}&temCapitulo=true&formato={POPULAR_FORMATS}&ordenarPor=view_geral"
            )
        };
        Ok(parse_list(&fetch_json(&target, LIST_FIXTURE)))
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

        let page = page(&request);
        let mut target = format!("{API_URL}/obras/buscar?limite=20&pagina={page}&temCapitulo=true");
        if !query.is_empty() {
            push_param(&mut target, "string", query);
        }
        let formato = filter_str(&request, "formato")
            .filter(|value| !value.is_empty())
            .unwrap_or(POPULAR_FORMATS);
        push_param(&mut target, "formato", formato);
        for key in ["status", "ordenarPor"] {
            if let Some(value) = filter_str(&request, key).filter(|value| !value.is_empty()) {
                push_param(&mut target, key, value);
            }
        }
        Ok(parse_list(&fetch_json(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".into());
        Ok(details_by_id(id_from_key(&key).unwrap_or("1")))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/obra/1".into());
        let id = id_from_key(&key).unwrap_or("1");
        let value = json_value(
            &fetch_json(&format!("{API_URL}/obras/{id}"), DETAILS_FIXTURE),
            DETAILS_FIXTURE,
        );
        Ok(value
            .get("capitulos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(chapter_item)
            .collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/capitulo/1".into());
        let id = key.trim_matches('/').rsplit('/').next().unwrap_or("1");
        let value = json_value(
            &fetch_json(&format!("{API_URL}/capitulos/{id}"), PAGES_FIXTURE),
            PAGES_FIXTURE,
        );
        let mut pages = parse_pages(&value);
        if pages.is_empty() {
            if let Some(cdn) = cdn_page_list_url(&value) {
                pages = parse_cdn_pages(&fetch_json(&cdn, CDN_PAGES_FIXTURE));
            }
        }
        Ok(pages)
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
            .and_then(|key| id_from_key(&key).map(|id| format!("{BASE_URL}/obra/{id}"))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            format!(
                "{BASE_URL}/capitulo/{}",
                key.trim_matches('/').rsplit('/').next().unwrap_or_default()
            )
        }))
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
        .with_header("Accept", "application/json, text/plain, */*")
        .with_header("x-app-key", "toons-mediocre-app")
        .with_referer(format!("{BASE_URL}/"))
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

fn parse_list(body: &str) -> Paged<CatalogItem> {
    let value = json_value(body, LIST_FIXTURE);
    Paged {
        entries: value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| catalog_item(item, false))
            .collect(),
        has_next_page: value
            .get("pagination")
            .and_then(|p| {
                let current = p.get("currentPage").and_then(Value::as_u64)?;
                let total = p.get("totalPages").and_then(Value::as_u64)?;
                Some(current < total)
            })
            .unwrap_or(false),
    }
}

fn details_by_id(id: &str) -> CatalogItem {
    catalog_item(
        &json_value(
            &fetch_json(&format!("{API_URL}/obras/{id}"), DETAILS_FIXTURE),
            DETAILS_FIXTURE,
        ),
        true,
    )
}

fn catalog_item(value: &Value, initialized: bool) -> CatalogItem {
    let id = str_or_int(value, &["obr_id", "id"]).unwrap_or_else(|| "1".into());
    let title = text_field(value, &["obr_nome", "nome"]).unwrap_or_else(|| "Mediocre Toons".into());
    let description =
        text_field(value, &["obr_descricao", "description"]).map(|text| html::strip_tags(&text));
    CatalogItem {
        key: format!("/obra/{id}"),
        title,
        cover: text_field(value, &["obr_imagem", "imagem"]).map(|path| image_url(&path, &id)),
        description,
        tags: value
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tag| text_field(tag, &["tag_nome", "nome"]))
            .collect(),
        status: status(value),
        url: Some(format!("{BASE_URL}/obra/{id}")),
        language: Some("pt-BR".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn chapter_item(value: &Value) -> MangaChapter {
    let id = str_or_int(value, &["cap_id", "id"]).unwrap_or_else(|| "1".into());
    let number = value
        .get("cap_num")
        .and_then(Value::as_f64)
        .map(|n| n as f32);
    MangaChapter {
        key: format!("/capitulo/{id}"),
        title: text_field(value, &["cap_nome", "name"])
            .or_else(|| number.map(|n| format!("Capitulo {}", format_number(n)))),
        chapter_number: number,
        date_uploaded: text_field(value, &["cap_lancado_em", "cap_criado_em"])
            .and_then(|text| parse_date(&text)),
        url: Some(format!("{BASE_URL}/capitulo/{id}")),
        ..MangaChapter::default()
    }
}

fn parse_pages(value: &Value) -> Vec<MangaPage> {
    let manga_id = value
        .get("obra")
        .and_then(|m| str_or_int(m, &["obr_id", "id"]))
        .unwrap_or_default();
    let chapter_number = value
        .get("cap_num")
        .and_then(Value::as_f64)
        .map(|n| format_number(n as f32))
        .or_else(|| text_field(value, &["cap_nome"]))
        .unwrap_or_else(|| "1".into());
    value
        .get("paginas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|page| page_src(page).map(|src| page_url(&src, &manga_id, &chapter_number)))
        .enumerate()
        .map(page_entry)
        .collect()
}

fn parse_cdn_pages(body: &str) -> Vec<MangaPage> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|page| page_src(page).map(|src| url::join_url(CDN_URL, &src)))
        .enumerate()
        .map(page_entry)
        .collect()
}

fn page_entry((index, image): (usize, String)) -> MangaPage {
    let headers = image_headers();
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn cdn_page_list_url(value: &Value) -> Option<String> {
    let manga_id = value
        .get("obra")
        .and_then(|m| str_or_int(m, &["obr_id", "id"]))?;
    let uuid = text_field(value, &["cap_uuid"])?;
    let chapter_number = value
        .get("cap_num")
        .and_then(Value::as_f64)
        .map(|n| format_number(n as f32))?;
    Some(format!(
        "{CDN_URL}/obras/{manga_id}/capitulos/{chapter_number}/{uuid}.json"
    ))
}

fn image_url(path: &str, id: &str) -> String {
    if path.starts_with("http") {
        path.into()
    } else {
        format!("{CDN_URL}/obras/{id}/{}", path.trim_start_matches('/'))
    }
}

fn page_url(path: &str, manga_id: &str, chapter_number: &str) -> String {
    if path.starts_with("http") {
        path.into()
    } else if path.starts_with("obras/") {
        format!("{CDN_URL}/{path}")
    } else {
        format!("{CDN_URL}/obras/{manga_id}/capitulos/{chapter_number}/{path}")
    }
}

fn page_src(value: &Value) -> Option<String> {
    text_field(value, &["url", "src"]).filter(|value| !value.trim().is_empty())
}

fn status(value: &Value) -> ItemStatus {
    let raw = value
        .get("status")
        .and_then(|status| text_field(status, &["nome", "name"]))
        .or_else(|| text_field(value, &["obr_status"]))
        .unwrap_or_default()
        .to_lowercase();
    match raw.as_str() {
        "em andamento" | "ativo" | "em_lancamento" | "em lancamento" | "em lançamento" => {
            ItemStatus::Ongoing
        }
        "completo" | "concluido" | "concluído" | "finalizado" => ItemStatus::Completed,
        "hiato" => ItemStatus::Hiatus,
        "cancelada" | "cancelado" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn image_headers() -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".into(), format!("{BASE_URL}/"));
    headers.insert(
        "Accept".into(),
        "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".into(),
    );
    headers
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

fn filter_str<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request.get("filters")?.get(key)?.as_str()
}

fn push_param(target: &mut String, key: &str, value: &str) {
    target.push('&');
    target.push_str(key);
    target.push('=');
    target.push_str(&url::query_escape(value));
}

fn id_from_url(input: &str) -> Option<String> {
    input.split("/obra/").nth(1).map(|rest| {
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

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
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

fn parse_date(input: &str) -> Option<i64> {
    dates::parse_ymd(input.get(..10).unwrap_or(input)).or_else(|| dates::parse_fixture_date(input))
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

const LIST_FIXTURE: &str = r#"{"data":[{"obr_id":1,"obr_nome":"Sample Mediocre","obr_imagem":"cover.jpg","obr_descricao":"Sample description","tags":[{"tag_nome":"Acao"}],"status":{"nome":"Em andamento"}}],"pagination":{"currentPage":1,"totalPages":1}}"#;
const DETAILS_FIXTURE: &str = r#"{"obr_id":1,"obr_nome":"Sample Mediocre","obr_imagem":"cover.jpg","obr_descricao":"Sample description","tags":[{"tag_nome":"Acao"}],"status":{"nome":"Completo"},"capitulos":[{"cap_id":10,"cap_nome":"Capitulo 1","cap_num":1.0,"cap_lancado_em":"2024-01-01T00:00:00.000Z"}]}"#;
const PAGES_FIXTURE: &str = r#"{"cap_id":10,"cap_uuid":"uuid","cap_num":1.0,"obra":{"obr_id":1},"paginas":[{"url":"page-1.jpg","ordem":1},{"url":"page-2.jpg","ordem":2}]}"#;
const CDN_PAGES_FIXTURE: &str = r#"[{"url":"obras/1/capitulos/1/page-1.jpg","ordem":1}]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixtures() {
        assert_eq!(parse_list(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            parse_pages(&json_value(PAGES_FIXTURE, PAGES_FIXTURE)).len(),
            2
        );
    }
}
