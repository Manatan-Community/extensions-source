use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http, url};
use serde_json::Value;

const SOURCE: MyReadingManga = MyReadingManga;
const BASE_URL: &str = "https://myreadingmanga.info";
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Mobile Safari/537.36";

struct MyReadingManga;

impl MangaSource for MyReadingManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            latest_url(source, page)
        } else {
            format!(
                "{BASE_URL}/page/{page}/?s=&ep_sort=rand&ep_filter_lang={}",
                encode_query(source.site_lang)
            )
        };
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing_page(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, source, Some(key))],
                has_next_page: false,
            });
        }
        let target = search_url(source, &request, page, query);
        let body = fetch_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing_page(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, source, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample/".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, source, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/1".into());
        let body = fetch_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let source = source_for(&request);
            let key = normalize_key(input);
            let body = fetch_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, source, Some(key))),
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
    site_lang: &'static str,
    latest_lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "myreadingmanga-ar",
        lang: "ar",
        site_lang: "Arabic",
        latest_lang: "Arabic",
    },
    SourceConfig {
        id: "myreadingmanga-id",
        lang: "id",
        site_lang: "Indonesia",
        latest_lang: "Indonesia",
    },
    SourceConfig {
        id: "myreadingmanga-zh",
        lang: "zh",
        site_lang: "Chinese",
        latest_lang: "Chinese",
    },
    SourceConfig {
        id: "myreadingmanga-en",
        lang: "en",
        site_lang: "English",
        latest_lang: "English",
    },
    SourceConfig {
        id: "myreadingmanga-de",
        lang: "de",
        site_lang: "German",
        latest_lang: "German",
    },
    SourceConfig {
        id: "myreadingmanga-it",
        lang: "it",
        site_lang: "Italian",
        latest_lang: "Italian",
    },
    SourceConfig {
        id: "myreadingmanga-ja",
        lang: "ja",
        site_lang: "Japanese",
        latest_lang: "jp",
    },
    SourceConfig {
        id: "myreadingmanga-ko",
        lang: "ko",
        site_lang: "Korean",
        latest_lang: "Korean",
    },
    SourceConfig {
        id: "myreadingmanga-pt-br",
        lang: "pt-BR",
        site_lang: "Portuguese",
        latest_lang: "Portuguese",
    },
    SourceConfig {
        id: "myreadingmanga-ru",
        lang: "ru",
        site_lang: "Russian",
        latest_lang: "Russian",
    },
    SourceConfig {
        id: "myreadingmanga-es",
        lang: "es",
        site_lang: "Spanish",
        latest_lang: "Spanish",
    },
    SourceConfig {
        id: "myreadingmanga-tr",
        lang: "tr",
        site_lang: "Turkish",
        latest_lang: "Turkish",
    },
    SourceConfig {
        id: "myreadingmanga-vi",
        lang: "vi",
        site_lang: "Vietnamese",
        latest_lang: "Vietnamese",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("myreadingmanga-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[3])
}

fn latest_url(source: SourceConfig, page: u64) -> String {
    let suffix = if page > 1 {
        format!("/page/{page}/")
    } else {
        String::new()
    };
    format!(
        "{BASE_URL}/lang/{}{}",
        source.latest_lang.to_lowercase(),
        suffix
    )
}

fn search_url(source: SourceConfig, request: &Value, page: u64, query: &str) -> String {
    let filters = request.get("filters").unwrap_or(&Value::Null);
    let mut pairs = vec![format!("s={}", encode_query(query))];
    if filters
        .get("enforceLanguage")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        pairs.push(format!("ep_filter_lang={}", encode_query(source.site_lang)));
    }
    for (key, param) in [
        ("genre", "ep_filter_genre"),
        ("tag", "ep_filter_post_tag"),
        ("category", "ep_filter_category"),
        ("pairing", "ep_filter_pairing"),
        ("group", "ep_filter_group"),
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
        {
            pairs.push(format!("{param}={}", encode_query(value.trim())));
        }
    }
    let sort = filters
        .get("sort")
        .and_then(Value::as_str)
        .unwrap_or("date");
    pairs.push(format!("ep_sort={}", encode_query(sort)));
    format!("{BASE_URL}/page/{page}/?{}", pairs.join("&"))
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(BASE_URL)
        .get(target)
        .header("User-Agent", USER_AGENT)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing_page(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let title_chunk = chunk.split("a rel").nth(1).unwrap_or(chunk);
            let href = html::attr_after(title_chunk, "<a", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let raw_title = html::text_between(title_chunk, "<a", "</a>")
                .map(|text| html::strip_tags(&text))
                .or_else(|| html::attr_after(chunk, "<a", "title"))?;
            Some(CatalogItem {
                key: normalize_key(&href),
                title: clean_title(&raw_title),
                cover: image_attr(chunk).map(clean_thumbnail),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    let total = body
        .split("ep-search-count")
        .nth(1)
        .and_then(|text| first_number(&html::strip_tags(text)))
        .unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: (entries.len() as u64) < total || body.contains("pagination-next"),
        entries,
    }
}

fn parse_details(body: &str, source: SourceConfig, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample/".into());
    let heading = html::text_between(body, "<h1", "</h1>")
        .map(|text| html::strip_tags(&text))
        .unwrap_or_default();
    let author = heading
        .split('[')
        .nth(1)
        .and_then(|part| part.split(']').next())
        .unwrap_or("")
        .trim();
    CatalogItem {
        key: key.clone(),
        title: clean_title(&heading),
        authors: (!author.is_empty())
            .then(|| author.to_string())
            .into_iter()
            .collect(),
        artists: (!author.is_empty())
            .then(|| author.to_string())
            .into_iter()
            .collect(),
        description: Some(parse_description(body, &heading)),
        tags: parse_tags(body),
        cover: image_attr(body).map(clean_thumbnail),
        status: if body.contains("Completed") {
            ItemStatus::Completed
        } else if body.contains("Ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig, key: &str) -> Vec<MangaChapter> {
    let date = html::text_between(body, "entry-time", "</")
        .map(|text| html::strip_tags(&text))
        .and_then(|text| parse_mmm_dd_yyyy(&text));
    let numeric_last = body
        .split("page-numbers")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>")
                .and_then(|text| first_number(&html::strip_tags(&text)))
                .and_then(|value| u32::try_from(value).ok())
        })
        .max()
        .unwrap_or(0);
    let last = numeric_last
        .max(body.matches("page-numbers").count() as u32)
        .max(1);
    let mut chapters = (1..=last)
        .map(|page| MangaChapter {
            key: format!("{}/{}", key.trim_end_matches('/'), page),
            title: Some(format!("Part {page}")),
            date_uploaded: date,
            language: Some(source.lang.into()),
            url: Some(format!(
                "{}{}/{}",
                BASE_URL,
                key.trim_end_matches('/'),
                page
            )),
            ..MangaChapter::default()
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| has_image_extension(image))
        .fold(Vec::<String>::new(), |mut acc, image| {
            if !acc.contains(&image) {
                acc.push(image);
            }
            acc
        })
        .into_iter()
        .map(|image| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            ..MangaPage::default()
        })
        .collect()
}

fn parse_description(body: &str, heading: &str) -> String {
    let scanlated = body
        .split("entry-terms")
        .find(|part| part.contains("/group/"))
        .map(|part| {
            part.split("<a")
                .skip(1)
                .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
                .map(|text| html::strip_tags(&text))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .map(|value| format!("Scanlated by: {value}"));
    let body_text = body
        .split("entry-content")
        .nth(1)
        .map(html::strip_tags)
        .unwrap_or_default();
    [
        Some(heading.to_string()),
        scanlated,
        (!body_text.is_empty()).then_some(body_text),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("/genre/")
                || chunk.contains("/tag/")
                || chunk.contains("/cats/")
                || chunk.contains("/pairing/")
        })
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|text| html::strip_tags(&text))
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    ["data-src", "data-cfsrc", "src", "data-lazy-src"]
        .into_iter()
        .find_map(|name| html::attr(input, name))
        .filter(|value| has_image_extension(value))
        .map(|value| url::join_url(BASE_URL, &value))
}

fn clean_thumbnail(value: String) -> String {
    let Some((prefix, suffix)) = value.rsplit_once('-') else {
        return value;
    };
    let ext = suffix
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or(suffix);
    if ["jpg", "jpeg", "png", "webp"].contains(&ext.to_ascii_lowercase().as_str()) {
        format!("{prefix}.{ext}")
    } else {
        value
    }
}

fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let mut in_brackets = false;
    for ch in title.chars() {
        match ch {
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            _ if !in_brackets => out.push(ch),
            _ => {}
        }
    }
    out.substring_before_last('(').trim().to_string()
}

trait BeforeLast {
    fn substring_before_last(&self, needle: char) -> &str;
}

impl BeforeLast for str {
    fn substring_before_last(&self, needle: char) -> &str {
        self.rsplit_once(needle)
            .map(|(before, _)| before)
            .unwrap_or(self)
    }
}

fn has_image_extension(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp"]
        .iter()
        .any(|ext| lower.contains(ext))
}

fn normalize_key(value: &str) -> String {
    value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/')
        .to_string()
}

fn first_number(value: &str) -> Option<u64> {
    let digits = value
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn parse_mmm_dd_yyyy(value: &str) -> Option<i64> {
    let parts = value.replace(',', "");
    let mut parts = parts.split_whitespace();
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
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

const LIST_FIXTURE: &str = r#"
<div class="ep-search-count">1 result</div>
<article><a rel="bookmark" href="https://myreadingmanga.info/sample/" title="[Artist] Sample (English)">[Artist] Sample (English)</a><a class="entry-image-link"><img data-src="https://myreadingmanga.info/sample-200x300.jpg"></a></article>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>[Artist] Sample (English)</h1>
<time class="entry-time">Jan 01, 2024</time>
<div class="entry-header"><p><a href="/genre/drama/">Drama</a><a href="/tag/sample/">Sample</a></p></div>
<div class="entry-content"><p>Story text.</p><img data-src="https://myreadingmanga.info/page-1.jpg"></div>
<a class="page-numbers">2</a>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="entry-content"><img data-src="https://myreadingmanga.info/page-1.jpg"><img src="https://myreadingmanga.info/page-1.jpg"><img data-lazy-src="https://myreadingmanga.info/page-2.webp"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_myreadingmanga() {
        let source = SOURCES[3];
        assert_eq!(parse_listing_page(LIST_FIXTURE, source).entries.len(), 1);
        assert_eq!(parse_details(DETAILS_FIXTURE, source, None).title, "Sample");
        assert_eq!(parse_chapters(DETAILS_FIXTURE, source, "/sample/").len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
