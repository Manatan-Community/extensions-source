use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use serde_json::Value;
use std::collections::BTreeMap;

const BASE_URL: &str = "https://mangatoon.mobi";
const SOURCE: MangaToon = MangaToon;

struct MangaToon;

impl MangaSource for MangaToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request_page(&request).saturating_sub(1);
        let listing = request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = if listing == "latest" {
            "new"
        } else if source.lang == "pt-BR" {
            "comic"
        } else {
            "hot"
        };
        let body = fetch_or_fixture(
            &format!(
                "{BASE_URL}/{}/genre/{path}?type=1&page={page}",
                source.url_lang
            ),
            LIST_FIXTURE,
        );
        Ok(parse_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, key, source)],
                has_next_page: false,
            });
        }
        let body = fetch_or_fixture(
            &format!(
                "{BASE_URL}/{}/search?word={}",
                source.url_lang,
                encode_query(query)
            ),
            SEARCH_FIXTURE,
        );
        Ok(parse_search(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/en/detail/sample".into());
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
        Ok(parse_details(&body, key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "/en/detail/sample".into());
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}/episodes"), CHAPTERS_FIXTURE);
        let mut chapters = parse_chapters(&body, source);
        if let Some(first_paid) = first_paid_chapter(&chapters) {
            chapters.truncate(first_paid);
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = request_key(&request, "chapter").unwrap_or_else(|| "/en/watch/sample/1".into());
        let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_or_fixture(&format!("{BASE_URL}{key}"), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, key, source)),
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
    url_lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "mangatoon-zh",
        lang: "zh",
        url_lang: "cn",
    },
    SourceConfig {
        id: "mangatoon-en",
        lang: "en",
        url_lang: "en",
    },
    SourceConfig {
        id: "mangatoon-id",
        lang: "id",
        url_lang: "id",
    },
    SourceConfig {
        id: "mangatoon-vi",
        lang: "vi",
        url_lang: "vi",
    },
    SourceConfig {
        id: "mangatoon-es",
        lang: "es",
        url_lang: "es",
    },
    SourceConfig {
        id: "mangatoon-pt-br",
        lang: "pt-BR",
        url_lang: "pt",
    },
    SourceConfig {
        id: "mangatoon-th",
        lang: "th",
        url_lang: "th",
    },
    SourceConfig {
        id: "mangatoon-fr",
        lang: "fr",
        url_lang: "fr",
    },
    SourceConfig {
        id: "mangatoon-ja",
        lang: "ja",
        url_lang: "ja",
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("mangatoon-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[1])
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_or_fixture(url: &str, fixture: &str) -> String {
    client()
        .get(url)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = blocks_between(body, "<a", "</a>")
        .into_iter()
        .filter(|block| block.contains("content-title") && block.contains("href="))
        .filter_map(|block| {
            let href = attr(&block, "href")?;
            let title = text_between(&block, "content-title").unwrap_or_else(|| "MangaToon".into());
            Some(item_from_parts(
                &title,
                &href,
                first_img(&block),
                source,
                false,
            ))
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("span next") || body.contains("class=\"next\""),
    }
}

fn parse_search(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = blocks_between(body, "recommend-item", "</div>")
        .into_iter()
        .filter_map(|block| {
            let href = attr(&block, "href")?;
            let title = text_between(&block, "recommend-comics-title")
                .unwrap_or_else(|| "MangaToon".into());
            Some(item_from_parts(
                &title,
                &href,
                first_img(&block),
                source,
                false,
            ))
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("span next") || body.contains("class=\"next\""),
    }
}

fn parse_details(body: &str, key: String, source: SourceConfig) -> CatalogItem {
    let author = text_between(body, "detail-author-name").map(|value| {
        value
            .split_once(": ")
            .map(|(_, tail)| tail)
            .unwrap_or(&value)
            .to_string()
    });
    let description = block_after(body, "detail-description-short")
        .map(|block| blocks_between(block, "<p", "</p>"))
        .unwrap_or_default()
        .into_iter()
        .map(|block| clean_text(&strip_tags(&block)))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let tags = text_between(body, "detail-tags-info")
        .map(|value| {
            value
                .split('/')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(title_case)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cover = first_img(block_after(body, "detail-img").unwrap_or_default());
    CatalogItem {
        key,
        title: text_between(body, "detail-title").unwrap_or_else(|| "MangaToon".into()),
        authors: author.into_iter().collect(),
        description: (!description.is_empty()).then_some(description),
        tags,
        status: status_from_text(&text_between(body, "detail-status").unwrap_or_default()),
        cover: cover.filter(|url| !url.contains("cartoon-big-images")),
        url: Some(format!(
            "{BASE_URL}{}",
            request_path_from_key(&request_key_value(body))
        )),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    blocks_between(body, "episode-item-new", "</a>")
        .into_iter()
        .filter_map(|block| {
            let href = attr(&block, "href")?;
            let title =
                text_between(&block, "episode-title-new").unwrap_or_else(|| "Chapter".into());
            let number =
                text_between(&block, "episode-number").and_then(|value| value.parse().ok());
            Some(MangaChapter {
                key: request_path_from_key(&href),
                title: Some(title),
                chapter_number: number,
                date_uploaded: None,
                language: Some(source.lang.into()),
                url: Some(join_url(&href)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn first_paid_chapter(chapters: &[MangaChapter]) -> Option<usize> {
    for breakpoint in [5usize, 10, 15, 20] {
        if breakpoint > chapters.len() {
            continue;
        }
        let body = fetch_or_fixture(
            &format!("{BASE_URL}{}", chapters[breakpoint - 1].key),
            PAGES_FIXTURE,
        );
        if parse_pages(&body).is_empty() {
            return Some(breakpoint - 1);
        }
    }
    None
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    blocks_between(body, "<img", ">")
        .into_iter()
        .filter(|block| body[..body.find(block).unwrap_or(0)].contains("pictures"))
        .filter_map(|block| first_img(&block))
        .enumerate()
        .map(|(index, url)| {
            let mut headers = BTreeMap::new();
            headers.insert("Referer".into(), format!("{BASE_URL}/"));
            MangaPage {
                content: PageContent::Url {
                    url,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn item_from_parts(
    title: &str,
    href: &str,
    cover: Option<String>,
    source: SourceConfig,
    initialized: bool,
) -> CatalogItem {
    let key = request_path_from_key(href);
    CatalogItem {
        key: key.clone(),
        title: clean_text(title),
        cover: cover.map(|url| normal_poster_url(&url)),
        url: Some(join_url(&key)),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized,
        ..CatalogItem::default()
    }
}

fn request_key_value(body: &str) -> String {
    attr(body, "href").unwrap_or_else(|| "/en/detail/sample".into())
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get("key")
        .or_else(|| request.get(field).and_then(|value| value.get("key")))
        .or_else(|| request.get(field).and_then(|value| value.get("id")))
        .and_then(Value::as_str)
        .map(request_path_from_key)
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn deeplink_key(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) {
        Some(request_path_from_key(input))
    } else {
        None
    }
}

fn request_path_from_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn join_url(path: &str) -> String {
    if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{BASE_URL}{}", request_path_from_key(path))
    }
}

fn encode_query(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}

fn status_from_text(input: &str) -> ItemStatus {
    let text = input.trim().to_ascii_lowercase();
    if [
        "on going",
        "sedang berlangsung",
        "tiếp tục cập nhật",
        "en proceso",
        "atualizando",
        "en cours",
    ]
    .contains(&text.as_str())
        || input.contains("连载")
        || input.contains("セリアル")
        || input.contains("連載中")
    {
        ItemStatus::Ongoing
    } else if [
        "completed",
        "tamat",
        "đã full",
        "terminada",
        "concluído",
        "fin",
    ]
    .contains(&text.as_str())
        || input.contains("完结")
        || input.contains("จบ")
    {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn first_img(block: &str) -> Option<String> {
    attr(block, "data-src").or_else(|| attr(block, "src"))
}

fn normal_poster_url(input: &str) -> String {
    input
        .replace("jpg-poster", "jpg")
        .replace("png-poster", "png")
        .replace("webp-poster", "webp")
}

fn block_after<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    body.find(marker).map(|index| &body[index..])
}

fn blocks_between(body: &str, start: &str, end: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start_index) = rest.find(start) {
        rest = &rest[start_index..];
        let Some(end_index) = rest.find(end) else {
            break;
        };
        let block = &rest[..end_index + end.len()];
        out.push(block.to_string());
        rest = &rest[end_index + end.len()..];
    }
    out
}

fn text_between(body: &str, class_name: &str) -> Option<String> {
    let start = body.find(class_name)?;
    let after = &body[start..];
    let tag_end = after.find('>')?;
    let after_tag = &after[tag_end + 1..];
    let end = after_tag.find("</div>").or_else(|| after_tag.find('<'))?;
    Some(clean_text(&strip_tags(&after_tag[..end])))
}

fn attr(block: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = block.find(&needle)? + needle.len();
    let quote = block[start..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value_start = start + quote.len_utf8();
    let value_end = block[value_start..].find(quote)? + value_start;
    Some(clean_text(&block[value_start..value_end]))
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
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case(input: &str) -> String {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

const LIST_FIXTURE: &str = r#"
<div class="genre-content"><div class="items">
<a href="https://mangatoon.mobi/en/detail/sample"><img data-src="https://cdn.example/title.jpg-poster-123"><div class="content-title">Sample Toon</div></a>
</div></div><span class="next"></span>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="comics-result">
<div class="recommend-item"><a href="https://mangatoon.mobi/en/detail/sample"><img src="https://cdn.example/search.jpg-poster-1"><div class="recommend-comics-title">Search Toon</div></a></div>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<a href="/en/detail/sample"></a>
<div class="detail-title">Sample Toon</div>
<div class="detail-author-name"><span>Author: Creator</span></div>
<div class="detail-description-short"><p>First paragraph.</p><p>Second paragraph.</p></div>
<div class="detail-tags-info"><span>action/fantasy</span></div>
<div class="detail-status">Completed</div>
<div class="detail-img"><img src="https://cdn.example/cover.jpg"></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<a class="episode-item-new" href="/en/watch/sample/1"><div class="episode-number">1</div><div class="episode-title-new">Episode 1</div><div class="episode-date"><span class="open-date">2024-01-01</span></div></a>
<a class="episode-item-new" href="/en/watch/sample/2"><div class="episode-number">2</div><div class="episode-title-new">Episode 2</div><div class="episode-date"><span class="open-date">2024-01-02</span></div></a>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="pictures"><div><img data-src="https://cdn.example/page-1.jpg"></div></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_search_details_chapters_pages() {
        assert_eq!(
            parse_listing(LIST_FIXTURE, SOURCES[1]).entries[0].title,
            "Sample Toon"
        );
        assert_eq!(
            parse_search(SEARCH_FIXTURE, SOURCES[1]).entries[0].title,
            "Search Toon"
        );
        let details = parse_details(DETAILS_FIXTURE, "/en/detail/sample".into(), SOURCES[1]);
        assert_eq!(details.authors, vec!["Creator"]);
        assert_eq!(details.tags, vec!["Action", "Fantasy"]);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, SOURCES[1]).len(), 2);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
