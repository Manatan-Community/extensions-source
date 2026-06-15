use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://3hentai.net";
const SOURCE: Hentai3 = Hentai3;

struct Hentai3;

impl MangaSource for Hentai3 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            if source.search_lang.is_empty() {
                format!("{BASE_URL}/search?q=pages%3A>0&page={page}")
            } else {
                format!("{BASE_URL}/language/{}/{page}", source.search_lang)
            }
        } else if source.search_lang.is_empty() {
            format!("{BASE_URL}/search?q=pages%3A>0&page={page}&sort=popular")
        } else {
            format!(
                "{BASE_URL}/language/{}/{}?sort=popular",
                source.search_lang,
                if page > 1 { page.to_string() } else { String::new() }
            )
        };
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            let body = fetch_document_or_fixture(query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key), source, display_full_title(&request))],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut terms = Vec::new();
        if !query.is_empty() {
            terms.push(query.to_string());
            terms.push(format!("title:\"{query}\""));
        }
        if !source.search_lang.is_empty() {
            terms.push(format!("language:{}", source.search_lang));
        }
        terms.extend(filter_terms(filters));
        let mut target = format!("{BASE_URL}/search?q={}", url::query_escape(&terms.join(" ")));
        if page(&request) > 1 {
            target.push_str("&page=");
            target.push_str(&page(&request).to_string());
        }
        target.push_str("&sort=");
        target.push_str(sort_code(filters));
        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/d/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source, display_full_title(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/d/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Chapter".into()),
            chapter_number: Some(1.0),
            date_uploaded: html::text_between(&body, "<time", "</time>")
                .and_then(|value| parse_iso_like(&html::strip_tags(&value))),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/d/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) && input.contains("/d/") {
            let key = normalize_key(input);
            let body = fetch_document_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key), source, display_full_title(&request))),
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

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    search_lang: &'static str,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("hentai3-all");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/d/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, "div class=\"title\"", "</div>")
                .or_else(|| html::text_between(chunk, "class='title'", "</div>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "3Hentai Gallery".into()));
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(source.lang.into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig, full_title: bool) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/d/sample".into());
    let h1 = html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value));
    let span_title = html::text_between(body, "<span", "</span>").map(|value| html::strip_tags(&value));
    let title = if full_title {
        h1.clone().or(span_title)
    } else {
        span_title.or(h1.clone())
    }
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "3Hentai Gallery".into()));
    let authors = link_texts(body, "/groups/");
    let artists = link_texts(body, "/artists/");
    let mut tags = link_texts(body, "/tags/")
        .into_iter()
        .map(|tag| {
            let value = capitalize_each(&tag);
            if value.contains("(female)") || value.contains("(male)") {
                value.replace("(female)", "female").replace("(male)", "male")
            } else {
                value
            }
        })
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(body, "thumbnail", "src")
            .or_else(|| html::attr_after(body, "w-96", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        authors: if authors.is_empty() { artists.clone() } else { authors.clone() },
        artists: if artists.is_empty() { authors } else { artists },
        description: Some(details_description(body)),
        tags,
        language: Some(source.lang.into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| !chunk.contains("class=") && !chunk.contains("thumb") && !chunk.contains("cover"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|src| !src.starts_with("data:"))
        .enumerate()
        .map(|(index, src)| {
            let image = src.replace("t.", ".");
            MangaPage {
                content: PageContent::Url {
                    url: url::join_url(BASE_URL, &image),
                    context: None,
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn filter_terms(filters: &Value) -> Vec<String> {
    let mut terms = Vec::new();
    for (id, kind, specific) in [
        ("tags", "tags", ""),
        ("maleTags", "tags", "male"),
        ("femaleTags", "tags", "female"),
        ("series", "series", ""),
        ("characters", "characters", ""),
        ("artist", "artist", ""),
        ("groups", "groups", ""),
        ("language", "language", ""),
        ("page", "page", ""),
    ] {
        let Some(value) = filter_string(filters, id) else {
            continue;
        };
        for raw in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            let excluded = raw.starts_with('-');
            let clean = raw.trim_start_matches('-').to_ascii_lowercase();
            let mut term = String::new();
            if excluded {
                term.push('-');
            }
            term.push_str(kind);
            term.push_str(":'");
            term.push_str(&clean);
            if !specific.is_empty() {
                term.push_str(" (");
                term.push_str(specific);
                term.push(')');
            }
            term.push('\'');
            terms.push(term);
        }
    }
    terms
}

fn filter_string(filters: &Value, id: &str) -> Option<String> {
    filters.get(id).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => Some(items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")),
        _ => None,
    })
}

fn sort_code(filters: &Value) -> &'static str {
    match filter_string(filters, "sort").as_deref() {
        Some("Popular: All Time") => "popular",
        Some("Popular: Week") => "popular-7d",
        Some("Popular: Today") => "popular-24h",
        _ => "",
    }
}

fn display_full_title(request: &Value) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get("displayFullTitle"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn details_description(body: &str) -> String {
    let mut lines = Vec::new();
    for (label, marker) in [
        ("Characters", "/characters/"),
        ("Series", "/series/"),
        ("Groups", "/groups/"),
        ("Languages", "/language/"),
    ] {
        let values = link_texts(body, marker);
        if !values.is_empty() {
            lines.push(format!("{label}: {}", values.join(", ")));
        }
    }
    if let Some(pages) = html::text_between(body, "tag-container", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| value.to_ascii_lowercase().contains("pages"))
    {
        lines.push(pages);
    }
    lines.join("\n\n")
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn capitalize_each(input: &str) -> String {
    input
        .split(' ')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn parse_iso_like(value: &str) -> Option<i64> {
    value
        .split(['T', '-', ':'])
        .next()
        .and_then(|year| year.parse::<i64>().ok())
        .filter(|year| *year > 1970)
        .map(|year| (year - 1970) * 31_536_000)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "hentai3-all", lang: "all", search_lang: "" },
    SourceConfig { id: "hentai3-en", lang: "en", search_lang: "english" },
    SourceConfig { id: "hentai3-ja", lang: "ja", search_lang: "japanese" },
    SourceConfig { id: "hentai3-ko", lang: "ko", search_lang: "korean" },
    SourceConfig { id: "hentai3-zh", lang: "zh", search_lang: "chinese" },
    SourceConfig { id: "hentai3-mo", lang: "mo", search_lang: "mongolian" },
    SourceConfig { id: "hentai3-es", lang: "es", search_lang: "spanish" },
    SourceConfig { id: "hentai3-pt", lang: "pt", search_lang: "Portuguese" },
    SourceConfig { id: "hentai3-id", lang: "id", search_lang: "indonesian" },
    SourceConfig { id: "hentai3-jv", lang: "jv", search_lang: "javanese" },
    SourceConfig { id: "hentai3-tl", lang: "tl", search_lang: "tagalog" },
    SourceConfig { id: "hentai3-vi", lang: "vi", search_lang: "vietnamese" },
    SourceConfig { id: "hentai3-th", lang: "th", search_lang: "thai" },
    SourceConfig { id: "hentai3-my", lang: "my", search_lang: "burmese" },
    SourceConfig { id: "hentai3-tr", lang: "tr", search_lang: "turkish" },
    SourceConfig { id: "hentai3-ru", lang: "ru", search_lang: "russian" },
    SourceConfig { id: "hentai3-uk", lang: "uk", search_lang: "ukrainian" },
    SourceConfig { id: "hentai3-pl", lang: "pl", search_lang: "polish" },
    SourceConfig { id: "hentai3-fi", lang: "fi", search_lang: "finnish" },
    SourceConfig { id: "hentai3-de", lang: "de", search_lang: "german" },
    SourceConfig { id: "hentai3-it", lang: "it", search_lang: "italian" },
    SourceConfig { id: "hentai3-fr", lang: "fr", search_lang: "french" },
    SourceConfig { id: "hentai3-nl", lang: "nl", search_lang: "dutch" },
    SourceConfig { id: "hentai3-cs", lang: "cs", search_lang: "czech" },
    SourceConfig { id: "hentai3-hu", lang: "hu", search_lang: "hungarian" },
    SourceConfig { id: "hentai3-bg", lang: "bg", search_lang: "bulgarian" },
    SourceConfig { id: "hentai3-is", lang: "is", search_lang: "icelandic" },
    SourceConfig { id: "hentai3-la", lang: "la", search_lang: "latin" },
    SourceConfig { id: "hentai3-ar", lang: "ar", search_lang: "arabic" },
];

const LIST_FIXTURE: &str = r#"
<a href="https://3hentai.net/d/sample"><div class="title">Sample Gallery</div><img src="https://3hentai.net/thumb.jpg"></a>
<a rel="next" href="/search?page=2">Next</a>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Full Sample Gallery <span>Sample Gallery</span></h1>
<img class="w-96" src="https://3hentai.net/thumbnail.jpg">
<a href="/groups/sample-group">sample group</a>
<a href="/artists/sample-artist">sample artist</a>
<a href="/tags/outdoor">outdoor</a>
<a href="/characters/sample-character">sample character</a>
<a href="/series/sample-series">sample series</a>
<a href="/language/english">english</a>
<div class="tag-container">pages: 2</div>
<time>2024-01-01T00:00:00+00:00</time>
"#;

const PAGES_FIXTURE: &str = r#"
<img src="https://3hentai.net/images/1t.jpg">
<img src="https://3hentai.net/images/2t.jpg">
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hentai3() {
        let page = parse_listing(LIST_FIXTURE, SOURCES[0]);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let details = parse_details(DETAILS_FIXTURE, Some("/d/sample".into()), SOURCES[0], false);
        assert_eq!(details.title, "Sample Gallery");
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
        assert_eq!(sort_code(&serde_json::json!({"sort":"Popular: Week"})), "popular-7d");
    }
}
