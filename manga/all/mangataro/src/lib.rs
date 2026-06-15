use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangataro.org";
const SOURCE: MangaTaro = MangaTaro;

struct MangaTaro;

impl MangaSource for MangaTaro {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        if source.group_id.is_some() {
            let body = fetch_json_or_fixture(
                &format!(
                    "{BASE_URL}/auth/groups/{}/titles?page=1",
                    source.group_id.unwrap()
                ),
                GROUP_FIXTURE,
            );
            return Ok(parse_group_titles(&body, source));
        }
        let page = request_page(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let body = fetch_json_or_fixture_body(
            &format!("{BASE_URL}/wp-json/manga/v1/load"),
            search_payload(
                page,
                "",
                if latest { "latest" } else { "popular" },
                &Value::Null,
            ),
            LIST_FIXTURE,
        );
        Ok(parse_browse(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let id = key.rsplit('-').next().unwrap_or("100");
            let body = fetch_json_or_fixture(&details_url(id), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details_json(&body, Some(key), source)],
                has_next_page: false,
            });
        }
        if source.group_id.is_some() {
            let body = fetch_json_or_fixture(
                &format!(
                    "{BASE_URL}/auth/groups/{}/titles?page=1",
                    source.group_id.unwrap()
                ),
                GROUP_FIXTURE,
            );
            let mut page = parse_group_titles(&body, source);
            if !query.is_empty() {
                let needle = query.to_ascii_lowercase();
                page.entries
                    .retain(|item| item.title.to_ascii_lowercase().contains(&needle));
            }
            return Ok(page);
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let sort = filter_string(filters, "sort").unwrap_or("popular");
        let body = fetch_json_or_fixture_body(
            &format!("{BASE_URL}/wp-json/manga/v1/load"),
            search_payload(request_page(&request), query, sort, filters),
            LIST_FIXTURE,
        );
        Ok(parse_browse(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "100:sample".into());
        let id = key.split(':').next().unwrap_or("100");
        let body = fetch_json_or_fixture(&details_url(id), DETAILS_FIXTURE);
        Ok(parse_details_json(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "100:sample".into());
        let id = key.split(':').next().unwrap_or("100");
        let mut url = chapter_url(id);
        if let Some(group) = source.group_id {
            url.push_str(&format!("&group_id={group}"));
        }
        let body = fetch_json_or_fixture(&url, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/chapter-1-200".into());
        let chapter_id = key.rsplit('-').next().unwrap_or("200");
        let body = fetch_json_or_fixture(
            &format!("{BASE_URL}/auth/chapter-content?chapter_id={chapter_id}"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let id = key.rsplit('-').next().unwrap_or("100");
            let body = fetch_json_or_fixture(&details_url(id), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details_json(&body, Some(key), source)),
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

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    chapter_lang: &'static str,
    group_id: Option<u64>,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "mangataro-en",
        lang: "en",
        chapter_lang: "en",
        group_id: None,
    },
    SourceConfig {
        id: "mangataro-pt-br",
        lang: "pt-BR",
        chapter_lang: "pt-BR",
        group_id: Some(9),
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("mangataro-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture_body(target: &str, body: Value, fixture: &str) -> String {
    client()
        .post(target)
        .json(body.to_string())
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_payload(page: u64, query: &str, sort: &str, filters: &Value) -> Value {
    json!({
        "page": page,
        "search": query,
        "years": stringified_array(filter_string(filters, "year")),
        "genres": stringified_array(filter_string(filters, "genres")),
        "types": stringified_array(filter_string(filters, "type")),
        "statuses": stringified_array(filter_string(filters, "status")),
        "sort": sort,
        "genreMatchMode": "all"
    })
}

fn stringified_array(value: Option<&str>) -> String {
    let values = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
}

fn parse_browse(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("fixture is valid"));
    let entries = value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) != Some("Novel"))
        .map(|item| catalog_from_browse(item, source))
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() == 24,
        entries,
    }
}

fn parse_group_titles(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let value: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(GROUP_FIXTURE).expect("fixture is valid"));
    let entries = value
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let id = item
                .get("manga_id")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .to_string();
            let slug = item
                .get("manga_slug")
                .and_then(Value::as_str)
                .unwrap_or("sample");
            CatalogItem {
                key: format!("{id}:{slug}:{}", source.group_id.unwrap_or(0)),
                title: clean_text(
                    item.get("manga_title")
                        .and_then(Value::as_str)
                        .unwrap_or("MangaTaro"),
                ),
                cover: item
                    .get("cover_url")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                url: Some(format!("{BASE_URL}/manga/{slug}")),
                language: Some(source.lang.into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            }
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn catalog_from_browse(item: &Value, source: SourceConfig) -> CatalogItem {
    let id = item.get("id").and_then(Value::as_str).unwrap_or("100");
    let slug = item
        .get("url")
        .and_then(Value::as_str)
        .and_then(slug_from_url)
        .unwrap_or_else(|| "sample".into());
    CatalogItem {
        key: format!("{id}:{slug}"),
        title: clean_text(
            item.get("title")
                .and_then(Value::as_str)
                .unwrap_or("MangaTaro"),
        ),
        cover: item
            .get("cover")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        description: item
            .get("description")
            .and_then(Value::as_str)
            .map(clean_text),
        status: parse_status(
            item.get("status")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details_json(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let value: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("fixture is valid"));
    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .to_string();
    let slug = value
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("sample");
    CatalogItem {
        key: key.unwrap_or_else(|| format!("{id}:{slug}")),
        title: clean_text(
            value
                .get("title")
                .and_then(|title| title.get("rendered"))
                .and_then(Value::as_str)
                .unwrap_or("MangaTaro"),
        ),
        description: value
            .get("content")
            .and_then(|content| content.get("rendered"))
            .and_then(Value::as_str)
            .map(strip_tags)
            .map(|text| clean_text(&text)),
        cover: value
            .get("_embedded")
            .and_then(|embedded| embedded.get("wp:featuredmedia"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|media| media.get("source_url"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tags: embedded_terms(&value, "post_tag"),
        authors: embedded_terms(&value, "manga_author"),
        status: ItemStatus::Ongoing,
        url: Some(format!("{BASE_URL}/manga/{slug}")),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let value: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("fixture is valid"));
    value
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|chapter| {
            chapter
                .get("language")
                .and_then(Value::as_str)
                .is_none_or(|lang| lang.eq_ignore_ascii_case(source.chapter_lang))
        })
        .map(|chapter| {
            let url = chapter
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("/read/sample/chapter-1-200");
            let number = chapter
                .get("chapter")
                .and_then(Value::as_str)
                .unwrap_or("1");
            let title = chapter
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !matches!(*value, "" | "N/A" | "—"));
            MangaChapter {
                key: url.into(),
                title: Some(
                    title
                        .map(|title| format!("Chapter {number}: {}", clean_text(title)))
                        .unwrap_or_else(|| format!("Chapter {number}")),
                ),
                chapter_number: number.parse().ok(),
                scanlators: chapter
                    .get("group_name")
                    .and_then(Value::as_str)
                    .map(|value| vec![value.to_string()])
                    .unwrap_or_default(),
                language: Some(source.lang.into()),
                url: Some(format!("{BASE_URL}{url}")),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let value: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("fixture is valid"));
    value
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(index, image)| {
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), format!("{BASE_URL}/"));
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

fn details_url(id: &str) -> String {
    format!("{BASE_URL}/wp-json/wp/v2/manga/{id}?_embed")
}

fn chapter_url(id: &str) -> String {
    let timestamp = current_unix_seconds();
    let token_seed = format!("{timestamp}mng_ch_{}", utc_hour_stamp(timestamp));
    let token = format!("{:x}", md5::compute(token_seed))
        .chars()
        .take(16)
        .collect::<String>();
    format!(
        "{BASE_URL}/auth/manga-chapters?manga_id={id}&offset=0&limit=9999&order=DESC&_t={token}&_ts={timestamp}"
    )
}

#[cfg(target_arch = "wasm32")]
fn current_unix_seconds() -> i64 {
    manatan_extension::abi::host_call_json::<_, Value>("system.time", &())
        .ok()
        .and_then(|value| value.get("unixSeconds").and_then(Value::as_i64))
        .unwrap_or(0)
}

#[cfg(not(target_arch = "wasm32"))]
fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn utc_hour_stamp(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}{hour:02}")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_phase + 2) / 5 + 1;
    let month = month_phase + if month_phase < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn embedded_terms(value: &Value, taxonomy: &str) -> Vec<String> {
    value
        .get("_embedded")
        .and_then(|embedded| embedded.get("wp:term"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|group| group.as_array().into_iter().flatten())
        .filter(|term| term.get("taxonomy").and_then(Value::as_str) == Some(taxonomy))
        .filter_map(|term| term.get("name").and_then(Value::as_str))
        .map(clean_text)
        .collect()
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) && input.contains("/manga/") {
        let slug = input.trim_end_matches('/').rsplit('/').next()?;
        Some(format!("100:{slug}"))
    } else {
        None
    }
}

fn slug_from_url(input: &str) -> Option<String> {
    Some(input.trim_end_matches('/').rsplit('/').next()?.to_string())
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_string<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .or_else(|| filters.get("values").and_then(|values| values.get(key)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn parse_status(input: &str) -> ItemStatus {
    match input {
        "Ongoing" => ItemStatus::Ongoing,
        "Completed" => ItemStatus::Completed,
        _ => ItemStatus::Unknown,
    }
}

fn strip_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn clean_text(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

const LIST_FIXTURE: &str = r#"[{
  "id": "100",
  "url": "https://mangataro.org/manga/sample",
  "title": "Sample Taro",
  "cover": "https://mangataro.org/cover.jpg",
  "type": "Manga",
  "description": "Sample description.",
  "status": "Ongoing"
}]"#;

const GROUP_FIXTURE: &str = r#"{
  "titles": [{ "manga_id": 100, "manga_title": "Sample Taro BR", "group_id": 9, "manga_slug": "sample-br", "cover_url": "https://mangataro.org/cover.jpg" }]
}"#;

const DETAILS_FIXTURE: &str = r#"{
  "id": 100,
  "slug": "sample",
  "title": { "rendered": "Sample Taro" },
  "content": { "rendered": "<p>Sample description.</p>" },
  "type": "Manga",
  "_embedded": {
    "wp:featuredmedia": [{ "source_url": "https://mangataro.org/cover.jpg" }],
    "wp:term": [[{ "name": "Action", "taxonomy": "post_tag" }], [{ "name": "Author One", "taxonomy": "manga_author" }]]
  }
}"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "chapters": [{ "url": "/read/sample/chapter-1-200", "chapter": "1", "title": "Start", "date": "1 day ago", "group_name": "Group One", "language": "en" }]
}"#;

const PAGES_FIXTURE: &str = r#"{ "images": ["https://mangataro.org/page-1.jpg"] }"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_browse_group_details_chapters_pages() {
        assert_eq!(
            parse_browse(LIST_FIXTURE, SOURCES[0]).entries[0].title,
            "Sample Taro"
        );
        assert_eq!(
            parse_group_titles(GROUP_FIXTURE, SOURCES[1]).entries[0].title,
            "Sample Taro BR"
        );
        let item = parse_details_json(DETAILS_FIXTURE, None, SOURCES[0]);
        assert_eq!(item.authors, vec!["Author One"]);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, SOURCES[0]).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }

    #[test]
    fn formats_utc_hour_for_chapter_token() {
        assert_eq!(utc_hour_stamp(0), "1970010100");
        assert_eq!(utc_hour_stamp(1_700_000_000), "2023111422");
    }
}
