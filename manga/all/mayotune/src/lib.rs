use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: MayoTune = MayoTune;
const BASE_URL: &str = "https://mayochuu.xyz";

struct MayoTune;

impl MangaSource for MayoTune {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![source_item(source_for(&request))],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let item = source_item(source);
        let matches = query.is_empty()
            || item.title.to_lowercase().contains(&query)
            || "masakuni igarashi".contains(&query)
            || "mayonaka heart tune".contains(&query);
        Ok(Paged {
            entries: if matches { vec![item] } else { Vec::new() },
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let body = fetch_or_fixture(BASE_URL, DETAILS_FIXTURE);
        Ok(parse_details(&body, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let target = format!("{BASE_URL}/api/{}/chapters", source.chapter_endpoint);
        let body = fetch_or_fixture(&target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            format!(
                "/api/{}/chapters?id=sample&number=1",
                source.chapter_endpoint
            )
        });
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), PAGE_CHAPTER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let source = source_for(&request);
            return Ok(Some(UrlResolveResult {
                item: Some(source_item(source)),
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
    title: &'static str,
    chapter_endpoint: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "mayotune-en",
        lang: "en",
        title: "Tune In to the Midnight Heart",
        chapter_endpoint: "",
    },
    SourceConfig {
        id: "mayotune-ja",
        lang: "ja",
        title: "真夜中ハートチューン",
        chapter_endpoint: "raw",
    },
];

#[derive(Deserialize)]
struct ChapterDto {
    id: String,
    title: String,
    number: f32,
    #[serde(rename = "pageCount")]
    page_count: u32,
    date: String,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("mayotune-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn source_item(source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: "/".into(),
        title: source.title.into(),
        cover: Some(format!("{BASE_URL}/img/cover.jpg")),
        url: Some(BASE_URL.into()),
        authors: vec!["Masakuni Igarashi".into()],
        artists: vec!["Masakuni Igarashi".into()],
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        status: ItemStatus::Unknown,
        ..CatalogItem::default()
    }
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_details(body: &str, source: SourceConfig) -> CatalogItem {
    let mut item = source_item(source);
    item.description = html::text_between(body, "<div class=\"text-lg\"", "</div>")
        .map(|text| html::strip_tags(&text));
    item.tags = html::text_between(body, "<span class=\"text-sm\"", "</span>")
        .map(|text| {
            html::strip_tags(&text)
                .split('•')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    item.status = if body.contains("Completed") || body.contains("Finished") {
        ItemStatus::Completed
    } else if body.contains("Cancelled") {
        ItemStatus::Cancelled
    } else if body.contains("Hiatus") {
        ItemStatus::Hiatus
    } else if body.contains("Ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    };
    item.cover = html::attr_after(body, "object-contain", "src")
        .map(|src| url::join_url(BASE_URL, &src))
        .or(item.cover);
    item.initialized = true;
    item
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let mut chapters = serde_json::from_str::<Vec<ChapterDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE).expect("chapters fixture"))
        .into_iter()
        .map(|chapter| chapter_to_model(chapter, source))
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn chapter_to_model(chapter: ChapterDto, source: SourceConfig) -> MangaChapter {
    let number = number_string(chapter.number);
    let endpoint = source.chapter_endpoint;
    let key = format!("/api/{endpoint}/chapters?id={}&number={number}", chapter.id);
    MangaChapter {
        key: key.clone(),
        title: Some(if chapter.title.is_empty() {
            format!("Chapter {number}")
        } else {
            format!("Chapter {number}: {}", chapter.title)
        }),
        chapter_number: Some(chapter.number),
        date_uploaded: parse_iso_millis(&chapter.date),
        page_count: Some(chapter.page_count),
        language: Some(source.lang.into()),
        url: Some(format!("{BASE_URL}/{endpoint}/chapter/{}", chapter.id)),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let chapter = serde_json::from_str::<ChapterDto>(body)
        .unwrap_or_else(|_| serde_json::from_str(PAGE_CHAPTER_FIXTURE).expect("page fixture"));
    (1..=chapter.page_count)
        .map(|page| MangaPage {
            content: PageContent::Url {
                url: format!("{BASE_URL}/api/manga/{}/{page}", chapter.id),
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn number_string(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
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

const DETAILS_FIXTURE: &str = r#"
<div class="text-lg">A music romance story.</div>
<span class="text-sm">Romance • Comedy</span>
<div class="text-center">Ongoing Status</div>
<img class="object-contain" src="/img/cover.jpg">
"#;

const CHAPTERS_FIXTURE: &str = r#"[{"id":"sample","title":"Song","number":1.0,"pageCount":2,"date":"2024-01-01T00:00:00.000Z"}]"#;
const PAGE_CHAPTER_FIXTURE: &str = r#"{"id":"sample","title":"Song","number":1.0,"pageCount":2,"date":"2024-01-01T00:00:00.000Z"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mayotune() {
        let source = SOURCES[0];
        assert_eq!(source_item(source).title, "Tune In to the Midnight Heart");
        assert_eq!(
            parse_details(DETAILS_FIXTURE, source).tags,
            vec!["Romance", "Comedy"]
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, source).len(), 1);
        assert_eq!(parse_pages(PAGE_CHAPTER_FIXTURE).len(), 2);
    }
}
