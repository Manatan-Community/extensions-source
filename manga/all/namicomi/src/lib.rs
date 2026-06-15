use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::http, url};
use serde_json::Value;

const SOURCE: NamiComi = NamiComi;
const WEB_URL: &str = "https://namicomi.com";
const API_URL: &str = "https://api.namicomi.com";
const CDN_URL: &str = "https://uploads.namicomi.com";
const LIMIT: u64 = 20;

struct NamiComi;

impl MangaSource for NamiComi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let order = if latest { "publishedAt" } else { "views" };
        let target = search_url(source, page, "", order, &Value::Null);
        let body = fetch_json_or_fixture(&target, MANGA_LIST_FIXTURE);
        Ok(parse_manga_list(&body, source, preferences(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = title_id_from_query(query) {
            let target = format!("{API_URL}/title/search?ids[]={id}&{}", include_params());
            let body = fetch_json_or_fixture(&target, MANGA_LIST_FIXTURE);
            return Ok(parse_manga_list(&body, source, preferences(&request)));
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let order = filters
            .get("sort")
            .and_then(Value::as_str)
            .unwrap_or("views");
        let target = search_url(source, page, query, order, filters);
        let body = fetch_json_or_fixture(&target, MANGA_LIST_FIXTURE);
        Ok(parse_manga_list(&body, source, preferences(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        let target = format!("{API_URL}/title/{key}?{}", include_params());
        let body = fetch_json_or_fixture(&target, MANGA_DETAILS_FIXTURE);
        Ok(parse_manga_detail(
            &body,
            source,
            &key,
            preferences(&request),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample-title".into());
        let prefs = preferences(&request);
        let target = format!(
            "{API_URL}/chapter?titleId={key}&includes[]=organization&limit=200&offset=0&translatedLanguages[]={}&order[volume]=desc&order[chapter]=desc",
            source.ext_lang
        );
        let body = fetch_json_or_fixture(&target, CHAPTER_LIST_FIXTURE);
        Ok(parse_chapter_list(&body, source, prefs))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample-chapter".into());
        let prefs = preferences(&request);
        let target = format!("{API_URL}/images/chapter/{key}?newQualities=true");
        let body = fetch_json_or_fixture(&target, PAGE_LIST_FIXTURE);
        Ok(parse_pages(&body, &key, prefs))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = title_id_from_query(input) {
            let source = source_for(&request);
            let target = format!("{API_URL}/title/search?ids[]={id}&{}", include_params());
            let body = fetch_json_or_fixture(&target, MANGA_LIST_FIXTURE);
            let item = parse_manga_list(&body, source, preferences(&request))
                .entries
                .into_iter()
                .next();
            return Ok(Some(UrlResolveResult {
                item,
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    ext_lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "namicomi-en",
        lang: "en",
        ext_lang: "en",
    },
    SourceConfig {
        id: "namicomi-ar",
        lang: "ar",
        ext_lang: "ar",
    },
    SourceConfig {
        id: "namicomi-bg",
        lang: "bg",
        ext_lang: "bg",
    },
    SourceConfig {
        id: "namicomi-ca",
        lang: "ca",
        ext_lang: "ca",
    },
    SourceConfig {
        id: "namicomi-zh-hans",
        lang: "zh-Hans",
        ext_lang: "zh-hans",
    },
    SourceConfig {
        id: "namicomi-zh-hant",
        lang: "zh-Hant",
        ext_lang: "zh-hant",
    },
    SourceConfig {
        id: "namicomi-hr",
        lang: "hr",
        ext_lang: "hr",
    },
    SourceConfig {
        id: "namicomi-cs",
        lang: "cs",
        ext_lang: "cs",
    },
    SourceConfig {
        id: "namicomi-da",
        lang: "da",
        ext_lang: "da",
    },
    SourceConfig {
        id: "namicomi-nl",
        lang: "nl",
        ext_lang: "nl",
    },
    SourceConfig {
        id: "namicomi-et",
        lang: "et",
        ext_lang: "et",
    },
    SourceConfig {
        id: "namicomi-fil",
        lang: "fil",
        ext_lang: "fil",
    },
    SourceConfig {
        id: "namicomi-fi",
        lang: "fi",
        ext_lang: "fi",
    },
    SourceConfig {
        id: "namicomi-fr",
        lang: "fr",
        ext_lang: "fr",
    },
    SourceConfig {
        id: "namicomi-de",
        lang: "de",
        ext_lang: "de",
    },
    SourceConfig {
        id: "namicomi-el",
        lang: "el",
        ext_lang: "el",
    },
    SourceConfig {
        id: "namicomi-he",
        lang: "he",
        ext_lang: "he",
    },
    SourceConfig {
        id: "namicomi-hi",
        lang: "hi",
        ext_lang: "hi",
    },
    SourceConfig {
        id: "namicomi-hu",
        lang: "hu",
        ext_lang: "hu",
    },
    SourceConfig {
        id: "namicomi-is",
        lang: "is",
        ext_lang: "is",
    },
    SourceConfig {
        id: "namicomi-ga",
        lang: "ga",
        ext_lang: "ga",
    },
    SourceConfig {
        id: "namicomi-id",
        lang: "id",
        ext_lang: "id",
    },
    SourceConfig {
        id: "namicomi-it",
        lang: "it",
        ext_lang: "it",
    },
    SourceConfig {
        id: "namicomi-ja",
        lang: "ja",
        ext_lang: "ja",
    },
    SourceConfig {
        id: "namicomi-ko",
        lang: "ko",
        ext_lang: "ko",
    },
    SourceConfig {
        id: "namicomi-lt",
        lang: "lt",
        ext_lang: "lt",
    },
    SourceConfig {
        id: "namicomi-ms",
        lang: "ms",
        ext_lang: "ms",
    },
    SourceConfig {
        id: "namicomi-ne",
        lang: "ne",
        ext_lang: "ne",
    },
    SourceConfig {
        id: "namicomi-no",
        lang: "no",
        ext_lang: "no",
    },
    SourceConfig {
        id: "namicomi-pa",
        lang: "pa",
        ext_lang: "pa",
    },
    SourceConfig {
        id: "namicomi-fa",
        lang: "fa",
        ext_lang: "fa",
    },
    SourceConfig {
        id: "namicomi-pl",
        lang: "pl",
        ext_lang: "pl",
    },
    SourceConfig {
        id: "namicomi-pt-br",
        lang: "pt-BR",
        ext_lang: "pt-br",
    },
    SourceConfig {
        id: "namicomi-pt",
        lang: "pt",
        ext_lang: "pt-pt",
    },
    SourceConfig {
        id: "namicomi-ru",
        lang: "ru",
        ext_lang: "ru",
    },
    SourceConfig {
        id: "namicomi-sk",
        lang: "sk",
        ext_lang: "sk",
    },
    SourceConfig {
        id: "namicomi-sl",
        lang: "sl",
        ext_lang: "sl",
    },
    SourceConfig {
        id: "namicomi-es-419",
        lang: "es-419",
        ext_lang: "es-419",
    },
    SourceConfig {
        id: "namicomi-es",
        lang: "es",
        ext_lang: "es-es",
    },
    SourceConfig {
        id: "namicomi-sv",
        lang: "sv",
        ext_lang: "sv",
    },
    SourceConfig {
        id: "namicomi-th",
        lang: "th",
        ext_lang: "th",
    },
    SourceConfig {
        id: "namicomi-tr",
        lang: "tr",
        ext_lang: "tr",
    },
    SourceConfig {
        id: "namicomi-uk",
        lang: "uk",
        ext_lang: "uk",
    },
];

#[derive(Clone, Copy)]
struct Prefs {
    cover_suffix: &'static str,
    data_saver: bool,
    show_locked: bool,
}

fn preferences(request: &Value) -> Prefs {
    let prefs = request.get("preferences").unwrap_or(&Value::Null);
    let cover_suffix = match prefs
        .get("coverQuality")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        ".512.jpg" => ".512.jpg",
        ".256.jpg" => ".256.jpg",
        _ => "",
    };
    Prefs {
        cover_suffix,
        data_saver: prefs
            .get("dataSaver")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        show_locked: prefs
            .get("showLockedChapters")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("namicomi-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn search_url(
    source: SourceConfig,
    page: u64,
    query: &str,
    order: &str,
    filters: &Value,
) -> String {
    let offset = LIMIT * page.saturating_sub(1);
    let mut parts = vec![
        format!("limit={LIMIT}"),
        format!("offset={offset}"),
        format!("order[{}]=desc", order),
        format!("availableTranslatedLanguages[]={}", source.ext_lang),
        include_params(),
    ];
    if !query.trim().is_empty() {
        parts.push(format!(
            "title={}",
            encode_query(&query.replace(|c: char| c.is_whitespace(), " "))
        ));
    }
    if let Some(status) = filters
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("publicationStatus[]={}", status));
    }
    if let Some(ratings) = filters.get("contentRating").and_then(Value::as_str) {
        for rating in ratings.split(',').filter(|s| !s.is_empty()) {
            parts.push(format!("contentRating[]={}", rating));
        }
    }
    format!("{API_URL}/title/search?{}", parts.join("&"))
}

fn include_params() -> String {
    [
        "cover_art",
        "organization",
        "tag",
        "primary_tag",
        "secondary_tag",
    ]
    .into_iter()
    .map(|include| format!("includes[]={include}"))
    .collect::<Vec<_>>()
    .join("&")
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(WEB_URL)
        .get(target)
        .origin(WEB_URL)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_list(body: &str, source: SourceConfig, prefs: Prefs) -> Paged<CatalogItem> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(MANGA_LIST_FIXTURE).unwrap());
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| manga_from_data(&entry, source, prefs))
        .collect::<Vec<_>>();
    let has_next_page = value
        .pointer("/meta/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_manga_detail(body: &str, source: SourceConfig, key: &str, prefs: Prefs) -> CatalogItem {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(MANGA_DETAILS_FIXTURE).unwrap());
    let data = value.get("data").unwrap_or(&value);
    let mut item = manga_from_data(data, source, prefs);
    item.key = key.to_string();
    item.initialized = true;
    item
}

fn manga_from_data(data: &Value, source: SourceConfig, prefs: Prefs) -> CatalogItem {
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let attr = data.get("attributes").unwrap_or(&Value::Null);
    let title = localized_string(attr.get("title"), source.ext_lang).unwrap_or_else(|| id.clone());
    let description = localized_string(attr.get("description"), source.ext_lang);
    let cover = data
        .get("relationships")
        .and_then(Value::as_array)
        .and_then(|relationships| {
            relationships
                .iter()
                .find(|rel| rel.get("type").and_then(Value::as_str) == Some("cover_art"))
                .and_then(|rel| rel.pointer("/attributes/fileName").and_then(Value::as_str))
        })
        .map(|file| format!("{CDN_URL}/covers/{id}/{file}{}", prefs.cover_suffix));
    let authors = data
        .get("relationships")
        .and_then(Value::as_array)
        .map(|relationships| {
            relationships
                .iter()
                .filter(|rel| rel.get("type").and_then(Value::as_str) == Some("organization"))
                .filter_map(|rel| {
                    rel.pointer("/attributes/name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tags = data
        .get("relationships")
        .and_then(Value::as_array)
        .map(|relationships| {
            relationships
                .iter()
                .filter(|rel| {
                    matches!(
                        rel.get("type").and_then(Value::as_str),
                        Some("tag" | "primary_tag" | "secondary_tag")
                    )
                })
                .filter_map(|rel| {
                    localized_string(rel.pointer("/attributes/name"), source.ext_lang)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    CatalogItem {
        key: id.clone(),
        title: title.clone(),
        cover,
        url: Some(format!(
            "{WEB_URL}/{}/title/{}/{}",
            source.ext_lang,
            id,
            slug(&title)
        )),
        authors,
        description,
        tags,
        language: Some(source.lang.into()),
        content_rating: Some(
            match attr
                .get("contentRating")
                .and_then(Value::as_str)
                .unwrap_or("safe")
            {
                "pornographic" | "erotica" => "adult",
                "suggestive" => "suggestive",
                _ => "safe",
            }
            .into(),
        ),
        status: match attr
            .get("publicationStatus")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
            "ongoing" => ItemStatus::Ongoing,
            "completed" => ItemStatus::Completed,
            "cancelled" => ItemStatus::Cancelled,
            "hiatus" => ItemStatus::Hiatus,
            _ => ItemStatus::Unknown,
        },
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_list(body: &str, source: SourceConfig, prefs: Prefs) -> Vec<MangaChapter> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTER_LIST_FIXTURE).unwrap());
    value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| chapter_from_data(&entry, source, prefs))
        .collect()
}

fn chapter_from_data(data: &Value, source: SourceConfig, prefs: Prefs) -> Option<MangaChapter> {
    let id = data.get("id")?.as_str()?.to_string();
    let attr = data.get("attributes").unwrap_or(&Value::Null);
    let locked = attr
        .get("isLocked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if locked && !prefs.show_locked {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(volume) = attr
        .get("volume")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("Vol.{volume}"));
    }
    if let Some(chapter) = attr
        .get("chapter")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("Ch.{chapter}"));
    }
    if let Some(name) = attr
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        if !parts.is_empty() {
            parts.push("-".into());
        }
        parts.push(name.into());
    }
    let mut title = if parts.is_empty() {
        "Chapter".into()
    } else {
        parts.join(" ")
    };
    if locked {
        title = format!("Locked {title}");
    }
    Some(MangaChapter {
        key: id.clone(),
        title: Some(title),
        date_uploaded: attr
            .get("publishAt")
            .and_then(Value::as_str)
            .and_then(parse_iso_millis),
        language: Some(source.lang.into()),
        is_locked: locked,
        url: Some(format!("{WEB_URL}/{}/chapter/{id}", source.ext_lang)),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str, chapter_id: &str, prefs: Prefs) -> Vec<MangaPage> {
    let value = serde_json::from_str::<Value>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGE_LIST_FIXTURE).unwrap());
    let Some(data) = value.get("data") else {
        return Vec::new();
    };
    let hash = data.get("hash").and_then(Value::as_str).unwrap_or_default();
    let base = data
        .get("baseUrl")
        .and_then(Value::as_str)
        .unwrap_or(CDN_URL);
    let quality = if prefs.data_saver { "low" } else { "source" };
    data.get(quality)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|page| {
            page.get("filename")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|file| MangaPage {
            content: PageContent::Url {
                url: format!("{base}/chapter/{chapter_id}/{hash}/{quality}/{file}"),
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn localized_string(value: Option<&Value>, lang: &str) -> Option<String> {
    let object = value?.as_object()?;
    object
        .get(lang)
        .or_else(|| object.get("en"))
        .or_else(|| object.values().next())
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn title_id_from_query(query: &str) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    if let Some(id) = query.strip_prefix("id:") {
        return (!id.is_empty()).then(|| id.to_string());
    }
    query
        .split("/title/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() > 100 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

fn parse_iso_millis(value: &str) -> Option<i64> {
    let date = value.split(['.', '+', 'Z']).next()?;
    let mut parts = date.split(['T', '-', ':']);
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let hour = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let minute = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let second = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64) * 1_000)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

const MANGA_LIST_FIXTURE: &str = r#"{"data":[{"id":"sample-title","attributes":{"title":{"en":"Sample Nami"},"description":{"en":"Description."},"publicationStatus":"ongoing","contentRating":"safe"},"relationships":[{"type":"cover_art","attributes":{"fileName":"cover.jpg"}},{"type":"organization","attributes":{"name":"Team"}},{"type":"tag","attributes":{"name":{"en":"Drama"},"group":"genre"}}]}],"meta":{"hasNextPage":false,"offset":0,"limit":20}}"#;
const MANGA_DETAILS_FIXTURE: &str = r#"{"data":{"id":"sample-title","attributes":{"title":{"en":"Sample Nami"},"description":{"en":"Description."},"publicationStatus":"ongoing","contentRating":"safe"},"relationships":[{"type":"cover_art","attributes":{"fileName":"cover.jpg"}}]}}"#;
const CHAPTER_LIST_FIXTURE: &str = r#"{"data":[{"id":"sample-chapter","attributes":{"volume":"1","chapter":"1","name":"Start","publishAt":"2024-01-01T00:00:00+000"}}],"meta":{"hasNextPage":false,"offset":0,"limit":200}}"#;
const PAGE_LIST_FIXTURE: &str = r#"{"data":{"hash":"hash","baseUrl":"https://uploads.namicomi.com","source":[{"filename":"1.jpg"}],"low":[{"filename":"1-low.jpg"}]}}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namicomi() {
        let source = SOURCES[0];
        let prefs = Prefs {
            cover_suffix: "",
            data_saver: false,
            show_locked: false,
        };
        assert_eq!(
            parse_manga_list(MANGA_LIST_FIXTURE, source, prefs).entries[0].title,
            "Sample Nami"
        );
        assert_eq!(
            parse_manga_detail(MANGA_DETAILS_FIXTURE, source, "sample-title", prefs).key,
            "sample-title"
        );
        assert_eq!(
            parse_chapter_list(CHAPTER_LIST_FIXTURE, source, prefs).len(),
            1
        );
        assert_eq!(
            parse_pages(PAGE_LIST_FIXTURE, "sample-chapter", prefs).len(),
            1
        );
    }
}
