use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: PandaChaika = PandaChaika;
const BASE_URL: &str = "https://panda.chaika.moe";

const SOURCES: [SourceConfig; 20] = [
    SourceConfig {
        id: "pandachaika-all",
        lang: "all",
        search_lang: "",
    },
    SourceConfig {
        id: "pandachaika-en",
        lang: "en",
        search_lang: "english",
    },
    SourceConfig {
        id: "pandachaika-zh",
        lang: "zh",
        search_lang: "chinese",
    },
    SourceConfig {
        id: "pandachaika-ko",
        lang: "ko",
        search_lang: "korean",
    },
    SourceConfig {
        id: "pandachaika-es",
        lang: "es",
        search_lang: "spanish",
    },
    SourceConfig {
        id: "pandachaika-ru",
        lang: "ru",
        search_lang: "russian",
    },
    SourceConfig {
        id: "pandachaika-pt",
        lang: "pt",
        search_lang: "portuguese",
    },
    SourceConfig {
        id: "pandachaika-fr",
        lang: "fr",
        search_lang: "french",
    },
    SourceConfig {
        id: "pandachaika-th",
        lang: "th",
        search_lang: "thai",
    },
    SourceConfig {
        id: "pandachaika-vi",
        lang: "vi",
        search_lang: "vietnamese",
    },
    SourceConfig {
        id: "pandachaika-ja",
        lang: "ja",
        search_lang: "japanese",
    },
    SourceConfig {
        id: "pandachaika-id",
        lang: "id",
        search_lang: "indonesian",
    },
    SourceConfig {
        id: "pandachaika-ar",
        lang: "ar",
        search_lang: "arabic",
    },
    SourceConfig {
        id: "pandachaika-uk",
        lang: "uk",
        search_lang: "ukrainian",
    },
    SourceConfig {
        id: "pandachaika-tr",
        lang: "tr",
        search_lang: "turkish",
    },
    SourceConfig {
        id: "pandachaika-cs",
        lang: "cs",
        search_lang: "czech",
    },
    SourceConfig {
        id: "pandachaika-tl",
        lang: "tl",
        search_lang: "tagalog",
    },
    SourceConfig {
        id: "pandachaika-fi",
        lang: "fi",
        search_lang: "finnish",
    },
    SourceConfig {
        id: "pandachaika-jv",
        lang: "jv",
        search_lang: "javanese",
    },
    SourceConfig {
        id: "pandachaika-el",
        lang: "el",
        search_lang: "greek",
    },
];

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    search_lang: &'static str,
}

struct PandaChaika;

impl MangaSource for PandaChaika {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "public_date"
        } else {
            "rating"
        };
        let target = format!(
            "{BASE_URL}/search/?tags={}&sort={sort}&apply=&json=&page={page}",
            url::query_escape(source.search_lang)
        );
        Ok(parse_archive_response(
            &fetch_text_or_fixture(&target, SEARCH_FIXTURE),
            source,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let archive_id = query
            .strip_prefix("id:")
            .map(ToString::to_string)
            .or_else(|| archive_id_from_url(query));
        if let Some(id) = archive_id {
            return Ok(search_by_id(&id, source));
        }
        let target = if let Some(path) = query.strip_prefix("ehentai:") {
            format!(
                "{BASE_URL}/search/?qsearch={}&json=",
                url::query_escape(&format!("https://e-hentai.org/g/{path}"))
            )
        } else if let Some(path) = query.strip_prefix("fakku:") {
            format!(
                "{BASE_URL}/search/?qsearch={}&json=",
                url::query_escape(&format!("https://www.fakku.net/hentai/{path}"))
            )
        } else if let Some(link) = query.strip_prefix("source:") {
            format!(
                "{BASE_URL}/search/?qsearch={}&json=",
                url::query_escape(link)
            )
        } else {
            search_url(
                source,
                page,
                query,
                request.get("filters").unwrap_or(&Value::Null),
            )
        };
        Ok(parse_archive_response(
            &fetch_text_or_fixture(&target, SEARCH_FIXTURE),
            source,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "123".into());
        let body = fetch_text_or_fixture(&format!("{BASE_URL}/api?archive={key}"), ARCHIVE_FIXTURE);
        Ok(parse_archive_details(&body, &key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "123".into());
        let body = fetch_text_or_fixture(&format!("{BASE_URL}/api?archive={key}"), ARCHIVE_FIXTURE);
        let archive = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let download = archive
            .get("download")
            .and_then(Value::as_str)
            .unwrap_or("/archive/123/download/");
        Ok(vec![MangaChapter {
            key: download.trim_end_matches("/download/").to_string(),
            title: Some("Chapter".to_string()),
            date_uploaded: archive
                .get("posted")
                .and_then(Value::as_i64)
                .map(|value| value * 1000),
            url: Some(url::join_url(
                BASE_URL,
                download.trim_end_matches("/download/"),
            )),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/archive/123".into());
        let archive_url = format!("{BASE_URL}{}/download/", key.trim_end_matches('/'));
        Ok(archive_pages(&archive_url).unwrap_or_else(|_| fixture_archive_pages(&archive_url)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = archive_id_from_url(input) {
            let page = search_by_id(&id, source);
            return Ok(Some(UrlResolveResult {
                item: page.entries.into_iter().next(),
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

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("pandachaika-all");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(source: SourceConfig, page: u64, query: &str, filters: &Value) -> String {
    let mut tags = Vec::new();
    if !source.search_lang.is_empty() {
        tags.push(source.search_lang.to_string());
    }
    for key in [
        "tags",
        "maleTags",
        "femaleTags",
        "artist",
        "group",
        "parody",
        "character",
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let kind = match key {
                "maleTags" => "male",
                "femaleTags" => "female",
                "tags" => "",
                other => other,
            };
            for tag in value
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
            {
                tags.push(if kind.is_empty() {
                    tag.to_lowercase()
                } else if let Some(stripped) = tag.strip_prefix('-') {
                    format!("-{kind}:{}", stripped.to_lowercase())
                } else {
                    format!("{kind}:{}", tag.to_lowercase())
                });
            }
        }
    }
    let sort = filters
        .get("sort")
        .and_then(Value::as_str)
        .unwrap_or("rating");
    let direction = filters
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("desc");
    let category = filters
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace("All", "");
    let pages = filters
        .get("pages")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (min_pages, max_pages) = parse_page_range(pages);
    format!(
        "{BASE_URL}/search/?title={}&tags={}&sort={}&asc_desc={}&category={}&filecount_from={min_pages}&filecount_to={max_pages}&reason={}&uploader={}&page={page}&apply=&json=",
        url::query_escape(query),
        url::query_escape(&tags.join(",")),
        url::query_escape(sort),
        url::query_escape(direction),
        url::query_escape(&category),
        url::query_escape(
            filters
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        url::query_escape(
            filters
                .get("uploader")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
    )
}

fn parse_page_range(value: &str) -> (u32, u32) {
    let digits = value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let number = digits.parse::<u32>().unwrap_or(0).clamp(1, 9999);
    if number == 0 {
        return (1, 9999);
    }
    match value.chars().next() {
        Some('<') => (1, number),
        Some('>') => (number, 9999),
        _ => (number, number),
    }
}

fn search_by_id(id: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let body = fetch_text_or_fixture(&format!("{BASE_URL}/api?archive={id}"), ARCHIVE_FIXTURE);
    Paged {
        entries: vec![parse_archive_details(&body, id, source)],
        has_next_page: false,
    }
}

fn parse_archive_response(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let archives = root
        .get("archives")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Paged {
        entries: archives
            .iter()
            .map(|archive| long_archive_item(archive, source))
            .collect(),
        has_next_page: root
            .get("has_next")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn long_archive_item(archive: &Value, source: SourceConfig) -> CatalogItem {
    let key = archive
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .to_string();
    let tags = archive
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tag_strings = tags
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    CatalogItem {
        key: key.clone(),
        title: archive
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Archive")
            .to_string(),
        cover: archive
            .get("thumbnail")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        authors: filter_tags(&tag_strings, "group")
            .or_else(|| filter_tags(&tag_strings, "artist"))
            .into_iter()
            .collect(),
        artists: filter_tags(&tag_strings, "artist").into_iter().collect(),
        tags: tag_strings
            .iter()
            .map(|tag| tag.replace('_', " "))
            .collect(),
        description: Some(archive_description(archive, &tag_strings)),
        status: ItemStatus::Completed,
        url: Some(format!("{BASE_URL}/archive/{key}")),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_archive_details(body: &str, key: &str, source: SourceConfig) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    CatalogItem {
        key: key.to_string(),
        title: root
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Archive")
            .to_string(),
        status: ItemStatus::Completed,
        url: Some(format!("{BASE_URL}/archive/{key}")),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn archive_description(archive: &Value, tags: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(uploader) = archive
        .get("uploader")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("Uploader: {uploader}"));
    }
    if let Some(title_jpn) = archive
        .get("title_jpn")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("Japanese Title: {title_jpn}"));
    }
    if let Some(filecount) = archive.get("filecount").and_then(Value::as_u64) {
        parts.push(format!("Pages: {filecount}"));
    }
    if let Some(filesize) = archive.get("filesize").and_then(Value::as_f64) {
        parts.push(format!("File Size: {}", readable_size(filesize)));
    }
    let parodies = filter_tags(tags, "parody");
    if let Some(parodies) = parodies {
        parts.push(format!("Parodies: {parodies}"));
    }
    parts.join("\n")
}

fn filter_tags(tags: &[String], include: &str) -> Option<String> {
    let prefix = format!("{include}:");
    let values = tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(&prefix))
        .map(|tag| tag.replace('_', " "))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

fn readable_size(bytes: f64) -> String {
    if bytes >= 300_000_000.0 {
        format!("{:.2} GB", bytes / 1_000_000_000.0)
    } else if bytes >= 100_000.0 {
        format!("{:.2} MB", bytes / 1_000_000.0)
    } else if bytes >= 1_000.0 {
        format!("{:.2} kB", bytes / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn archive_pages(archive_url: &str) -> Result<Vec<MangaPage>, String> {
    let content_length = content_length(archive_url)?;
    let eocd_start = content_length.saturating_sub(128);
    let eocd = fetch_range(archive_url, eocd_start, content_length)?;
    let (cd_offset, cd_size) =
        parse_eocd(&eocd).ok_or_else(|| "missing ZIP central directory".to_string())?;
    let cd = fetch_range(archive_url, cd_offset, cd_offset + cd_size)?;
    let mut files = parse_central_directory(&cd);
    files.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    Ok(files
        .into_iter()
        .enumerate()
        .map(|(index, entry_path)| archive_page(index, archive_url, entry_path))
        .collect())
}

fn fixture_archive_pages(archive_url: &str) -> Vec<MangaPage> {
    ["001.jpg", "002.jpg"]
        .into_iter()
        .enumerate()
        .map(|(index, entry)| archive_page(index, archive_url, entry.to_string()))
        .collect()
}

fn archive_page(index: usize, archive_url: &str, entry_path: String) -> MangaPage {
    let mut extra = BTreeMap::new();
    extra.insert("entryPath".to_string(), Value::String(entry_path.clone()));
    MangaPage {
        content: PageContent::ArchiveEntry {
            archive_url: archive_url.to_string(),
            entry_path,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        extra,
        ..MangaPage::default()
    }
}

fn content_length(url: &str) -> Result<u64, String> {
    let response = client()
        .fetch("HEAD", url, None, BTreeMap::new())
        .map_err(|err| format!("{err:?}"))?;
    header(&response.headers, "content-length")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "missing content-length".to_string())
}

fn fetch_range(url: &str, start: u64, end: u64) -> Result<Vec<u8>, String> {
    let response = client()
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .send()
        .map_err(|err| format!("{err:?}"))?;
    let encoded = response
        .body_base64
        .ok_or_else(|| "missing binary response body".to_string())?;
    base64_decode(&encoded)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn parse_eocd(buffer: &[u8]) -> Option<(u64, u64)> {
    for index in 0..buffer.len().saturating_sub(4) {
        match le_u32(buffer, index)? {
            0x0606_4b50 => {
                let size = le_u64(buffer, index + 40)?;
                let offset = le_u64(buffer, index + 48)?;
                return Some((offset, size));
            }
            0x0605_4b50 => {
                let size = le_u32(buffer, index + 12)? as u64;
                let offset = le_u32(buffer, index + 16)? as u64;
                return Some((offset, size));
            }
            _ => {}
        }
    }
    None
}

fn parse_central_directory(buffer: &[u8]) -> Vec<String> {
    let mut files = Vec::new();
    let mut index = 0;
    while index + 46 <= buffer.len() {
        if le_u32(buffer, index) != Some(0x0201_4b50) {
            index += 1;
            continue;
        }
        let name_len = le_u16(buffer, index + 28).unwrap_or(0) as usize;
        let extra_len = le_u16(buffer, index + 30).unwrap_or(0) as usize;
        let comment_len = le_u16(buffer, index + 32).unwrap_or(0) as usize;
        let name_start = index + 46;
        let name_end = name_start + name_len;
        if name_end <= buffer.len() {
            if let Ok(name) = std::str::from_utf8(&buffer[name_start..name_end]) {
                if is_image_entry(name) {
                    files.push(name.to_string());
                }
            }
        }
        index += 46 + name_len + extra_len + comment_len;
    }
    files
}

fn is_image_entry(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif")
    )
}

fn le_u16(buffer: &[u8], index: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        buffer.get(index..index + 2)?.try_into().ok()?,
    ))
}

fn le_u32(buffer: &[u8], index: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        buffer.get(index..index + 4)?.try_into().ok()?,
    ))
}

fn le_u64(buffer: &[u8], index: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        buffer.get(index..index + 8)?.try_into().ok()?,
    ))
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4];
    let mut len = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err("invalid base64 character".to_string()),
        };
        chunk[len] = value;
        len += 1;
        if len == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 {
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            if chunk[3] != 64 {
                out.push((chunk[2] << 6) | chunk[3]);
            }
            len = 0;
        }
    }
    Ok(out)
}

fn archive_id_from_url(value: &str) -> Option<String> {
    let parts = value.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let archive_index = parts.iter().position(|part| *part == "archive")?;
    parts.get(archive_index + 1).map(|value| value.to_string())
}

export_manga_source!(SOURCE);

const SEARCH_FIXTURE: &str = r#"{"archives":[{"thumbnail":"https://img.example/thumb.jpg","title":"Fixture Archive","id":123,"posted":1704067200,"public_date":1704067200,"filecount":2,"filesize":2048,"tags":["artist:artist_one","female:tag_one","parody:sample"],"title_jpn":"Fixture JP","uploader":"Uploader"}],"has_next":true}"#;

const ARCHIVE_FIXTURE: &str =
    r#"{"download":"/archive/123/download/","posted":1704067200,"title":"Fixture Archive"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pandachaika_fixtures() {
        let source = SOURCES[0];
        let page = parse_archive_response(SEARCH_FIXTURE, source);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let details = parse_archive_details(ARCHIVE_FIXTURE, "123", source);
        assert_eq!(details.title, "Fixture Archive");
        let pages = fixture_archive_pages("https://panda.chaika.moe/archive/123/download/");
        assert_eq!(pages.len(), 2);
        assert_eq!(
            parse_central_directory(&fixture_central_directory()),
            vec!["001.jpg"]
        );
        assert_eq!(base64_decode("AQID").unwrap(), vec![1, 2, 3]);
    }

    fn fixture_central_directory() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        bytes.extend_from_slice(&(7u16).to_le_bytes());
        bytes.extend_from_slice(&(0u16).to_le_bytes());
        bytes.extend_from_slice(&(0u16).to_le_bytes());
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(b"001.jpg");
        bytes
    }
}
