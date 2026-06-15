use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Senkognito = Senkognito;
const NAME: &str = "Senkognito";
const DEFAULT_BASE_URL: &str = "https://senkognito.com";
const DEFAULT_API: &str = "https://api.senkuro.me";
const APP_ID: &str = "5033164800100";
const PAGE_SIZE: u64 = 20;

struct Senkognito;

impl MangaSource for Senkognito {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, DEFAULT_BASE_URL));
        }
        let body = graphql(
            &request,
            SEARCH_QUERY,
            json!({"orderBy":{"field":"POPULARITY_SCORE","direction":"DESC"},"offset": offset(&request)}),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body, &base_url(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("http://") || query.starts_with("https://") || query.starts_with("slug:") {
            let key = normalize_key(query);
            let body = graphql(&request, DETAILS_QUERY, json!({"mangaId": id_from_key(&key)}), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key), &base_url(&request))], has_next_page: false });
        }
        let body = graphql(
            &request,
            SEARCH_QUERY,
            json!({"query": query, "orderBy":{"field":"POPULARITY_SCORE","direction":"DESC"},"offset": offset(&request)}),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body, &base_url(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-id,,sample".into());
        let body = graphql(&request, DETAILS_QUERY, json!({"mangaId": id_from_key(&key)}), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), &base_url(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-id,,sample".into());
        let body = graphql(&request, CHAPTERS_QUERY, json!({"mangaId": id_from_key(&key)}), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "sample-id,,sample,,chapter-id,,chapter-1".into());
        let parts = key_parts(&key);
        let body = graphql(
            &request,
            PAGES_QUERY,
            json!({"mangaId": parts.first().copied().unwrap_or_default(), "chapterId": parts.get(2).copied().unwrap_or_default()}),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body, &base_url(&request)))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{}/manga/{}", base_url(&request), slug_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let parts = key_parts(&key);
            format!("{}/manga/{}/chapters/{}", base_url(&request), parts.get(1).copied().unwrap_or_default(), parts.get(3).copied().unwrap_or_default())
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.contains("/manga/") {
            let key = normalize_key(input);
            let body = graphql(&request, DETAILS_QUERY, json!({"mangaId": id_from_key(&key)}), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), &base_url(&request))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(request: &Value) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("App-Id", APP_ID)
        .with_header("Content-Type", "application/json")
        .with_referer(format!("{}/", base_url(request)))
        .with_cookies_for(base_url(request))
        .with_webview_challenge_fallback()
}

fn graphql(request: &Value, query: &str, variables: Value, fixture: &str) -> String {
    client(request)
        .post(format!("{}/graphql", api_url(request)))
        .json(json!({"query": query, "variables": variables}).to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(_request: &Value) -> String { DEFAULT_BASE_URL.into() }

fn api_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("apiDomain")).and_then(Value::as_str)
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_API.to_string())
}

fn offset(request: &Value) -> u64 {
    PAGE_SIZE * request.get("page").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1)
}

fn normalize_key(value: &str) -> String {
    if let Some(slug) = value.strip_prefix("slug:") {
        return format!(",,{}", slug.trim_matches('/'));
    }
    let slug = value.split("/manga/").nth(1).unwrap_or(value).split('/').next().unwrap_or(value);
    format!(",,{slug}")
}

fn key_parts(key: &str) -> Vec<&str> { key.split(",,").collect() }
fn id_from_key(key: &str) -> &str { key_parts(key).first().copied().filter(|v| !v.is_empty()).unwrap_or(key) }
fn slug_from_key(key: &str) -> &str { key_parts(key).get(1).copied().filter(|v| !v.is_empty()).unwrap_or(key) }

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mangas = root.pointer("/data/mangaTachiyomiSearch/mangas").and_then(Value::as_array).cloned().unwrap_or_default();
    Paged {
        has_next_page: !mangas.is_empty(),
        entries: mangas.iter().map(|item| catalog_from_item(item, None, base, false)).collect(),
    }
}

fn parse_details(body: &str, key: Option<String>, base: &str) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let data = root.pointer("/data/mangaTachiyomiInfo").unwrap_or(&root);
    catalog_from_item(data, key, base, true)
}

fn catalog_from_item(item: &Value, key: Option<String>, base: &str, initialized: bool) -> CatalogItem {
    let id = text(item, "id").unwrap_or_else(|| "sample-id".into());
    let slug = text(item, "slug").unwrap_or_else(|| "sample".into());
    let key = key.unwrap_or_else(|| format!("{id},,{slug}"));
    let alt = localized_list(item.get("alternativeNames"));
    let mut description = String::new();
    if !alt.is_empty() {
        description.push_str("Альтернативные названия:\n");
        description.push_str(&alt.join(" / "));
        description.push_str("\n\n");
    }
    if let Some(desc) = item.pointer("/localizations").and_then(Value::as_array).and_then(|list| localized_value(list, "RU")) {
        description.push_str(&desc);
    }
    let type_name = label_for(TYPE_LIST, text(item, "type").as_deref());
    let age = label_for(AGE_LIST, text(item, "rating").as_deref());
    let formats = item.get("formats").and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).filter_map(|value| label_for(FORMAT_LIST, Some(value))).collect::<Vec<_>>();
    let labels = item.get("labels").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|label| label.get("titles").and_then(Value::as_array).and_then(|list| localized_value(list, "RU"))).collect::<Vec<_>>();
    CatalogItem {
        key: key.clone(),
        title: title(item),
        alternate_titles: alt,
        cover: item.pointer("/cover/original/url").and_then(Value::as_str).map(ToString::to_string),
        authors: staff(item, "STORY"),
        artists: staff(item, "ART"),
        description: (!description.trim().is_empty()).then(|| description.trim().to_string()),
        tags: [type_name.into_iter().collect::<Vec<_>>(), age.into_iter().collect(), formats, labels].concat(),
        status: parse_status(text(item, "status").as_deref()),
        url: Some(format!("{base}/manga/{}", slug_from_key(&key))),
        language: Some("ru".into()),
        content_rating: Some("adult".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let teams = root.pointer("/data/mangaTachiyomiChapters/teams").and_then(Value::as_array).cloned().unwrap_or_default();
    root.pointer("/data/mangaTachiyomiChapters/chapters").and_then(Value::as_array).into_iter().flatten().map(|chapter| {
        let id = text(chapter, "id").unwrap_or_default();
        let slug = text(chapter, "slug").unwrap_or_default();
        let number = text(chapter, "number").unwrap_or_default();
        let volume = text(chapter, "volume").unwrap_or_default();
        let name = text(chapter, "name").unwrap_or_default();
        let mut title = format!("{volume}. Глава {number}");
        if !name.is_empty() { title.push_str(&format!(" {name}")); }
        MangaChapter {
            key: format!("{manga_key},,{id},,{slug}"),
            title: Some(title),
            chapter_number: number.parse().ok(),
            volume_number: volume.parse().ok(),
            scanlators: team_names(chapter, &teams),
            date_uploaded: text(chapter, "createdAt").and_then(|v| parse_iso_date(&v)),
            language: Some("ru".into()),
            ..MangaChapter::default()
        }
    }).collect()
}

fn parse_pages(body: &str, base: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.pointer("/data/mangaTachiyomiChapterPages/pages").and_then(Value::as_array).into_iter().flatten().enumerate().filter_map(|(index, page)| {
        let image = text(page, "url")?;
        Some(MangaPage {
            content: PageContent::Url { url: image.clone(), context: Some(manga::image_headers(base)) },
            headers: manga::image_headers(base),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
    }).collect()
}

fn title(item: &Value) -> String {
    item.get("titles").and_then(Value::as_array).and_then(|list| localized_value(list, "RU"))
        .or_else(|| item.get("titles").and_then(Value::as_array).and_then(|list| localized_value(list, "EN")))
        .or_else(|| item.get("titles").and_then(Value::as_array).and_then(|list| list.first()).and_then(|v| text(v, "content")))
        .or_else(|| text(item, "slug").and_then(|slug| url::slug_from_url(&slug)))
        .unwrap_or_else(|| NAME.into())
}

fn localized_list(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(|v| text(v, "content")).collect()
}

fn localized_value(list: &[Value], lang: &str) -> Option<String> {
    list.iter().find(|v| text(v, "lang").as_deref() == Some(lang)).and_then(|v| text(v, "content").or_else(|| text(v, "description")))
}

fn staff(item: &Value, role: &str) -> Vec<String> {
    item.get("mainStaff").and_then(Value::as_array).into_iter().flatten()
        .filter(|staff| staff.get("roles").and_then(Value::as_array).into_iter().flatten().any(|value| value.as_str() == Some(role)))
        .filter_map(|staff| staff.pointer("/person/name").and_then(Value::as_str).map(ToString::to_string))
        .collect()
}

fn team_names(chapter: &Value, teams: &[Value]) -> Vec<String> {
    chapter.get("teamIds").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
        .filter_map(|id| teams.iter().find(|team| text(team, "id").as_deref() == Some(id)).and_then(|team| text(team, "name")))
        .collect()
}

fn label_for(list: &[(&str, &str)], value: Option<&str>) -> Option<String> {
    let value = value?;
    list.iter().find(|(id, _)| *id == value).map(|(_, name)| (*name).to_string())
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status {
        Some("FINISHED") => ItemStatus::Completed,
        Some("ONGOING") | Some("ANNOUNCE") => ItemStatus::Ongoing,
        Some("HIATUS") => ItemStatus::Hiatus,
        Some("CANCELLED") => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_ymd(date)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(ToString::to_string)
}

const TYPE_LIST: &[(&str, &str)] = &[("MANGA", "Манга"), ("MANHWA", "Манхва"), ("MANHUA", "Маньхуа"), ("COMICS", "Комикс"), ("OEL_MANGA", "OEL Манга"), ("RU_MANGA", "РуМанга")];
const AGE_LIST: &[(&str, &str)] = &[("GENERAL", "0+"), ("SENSITIVE", "12+"), ("QUESTIONABLE", "16+"), ("EXPLICIT", "18+")];
const FORMAT_LIST: &[(&str, &str)] = &[("DIGEST", "Сборник"), ("DOUJINSHI", "Додзинси"), ("IN_COLOR", "В цвете"), ("SINGLE", "Сингл"), ("WEB", "Веб"), ("WEBTOON", "Вебтун"), ("YONKOMA", "Ёнкома"), ("SHORT", "Short")];

const SEARCH_QUERY: &str = r#"query searchTachiyomiManga($query: String,$orderBy: MangaTachiyomiOrder,$offset: Int){mangaTachiyomiSearch(query:$query,orderBy:$orderBy,offset:$offset){mangas{id slug titles{lang content} alternativeNames{lang content} cover{original{url}}}}}"#;
const DETAILS_QUERY: &str = r#"query fetchTachiyomiManga($mangaId: ID!){mangaTachiyomiInfo(mangaId:$mangaId){id slug titles{lang content} alternativeNames{lang content} localizations{lang description} type rating status formats labels{id rootId slug titles{lang content}} cover{original{url}} mainStaff{roles person{name}}}}"#;
const CHAPTERS_QUERY: &str = r#"query fetchTachiyomiChapters($mangaId: ID!){mangaTachiyomiChapters(mangaId:$mangaId){message chapters{id slug branchId name teamIds number volume createdAt} teams{id slug name}}}"#;
const PAGES_QUERY: &str = r#"query fetchTachiyomiChapterPages($mangaId: ID!,$chapterId: ID!){mangaTachiyomiChapterPages(mangaId:$mangaId,chapterId:$chapterId){pages{url}}}"#;

const LIST_FIXTURE: &str = r#"{"data":{"mangaTachiyomiSearch":{"mangas":[{"id":"sample-id","slug":"sample","titles":[{"lang":"RU","content":"Пример"}],"cover":{"original":{"url":"https://senkuro.me/sample.jpg"}}]}}}"#;
const DETAILS_FIXTURE: &str = r#"{"data":{"mangaTachiyomiInfo":{"id":"sample-id","slug":"sample","status":"ONGOING","type":"MANGA","rating":"GENERAL","titles":[{"lang":"RU","content":"Пример"}],"alternativeNames":[],"localizations":[{"lang":"RU","description":"Описание"}],"cover":{"original":{"url":"https://senkuro.me/sample.jpg"}},"mainStaff":[]}}}"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":{"mangaTachiyomiChapters":{"chapters":[{"id":"chapter-id","slug":"chapter-1","branchId":"branch","name":"","teamIds":[],"number":"1","volume":"1","createdAt":"2024-01-01T00:00:00.0"}],"teams":[]}}}"#;
const PAGES_FIXTURE: &str = r#"{"data":{"mangaTachiyomiChapterPages":{"pages":[{"url":"https://senkuro.me/page.jpg"}]}}}"#;

export_manga_source!(SOURCE);
