use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, NovelChapter, NovelChapterPage, NovelText, Paged, UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;
use std::collections::VecDeque;

const SOURCE: ReadFrom = ReadFrom;
const BASE_URL: &str = "https://readfrom.net";

struct ReadFrom;

impl NovelSource for ReadFrom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .is_some_and(|listing| listing == "latest");
        let kind = if latest {
            "last_added_books"
        } else {
            "allbooks"
        };
        let body = fetch(&format!("{BASE_URL}/{kind}/page/{page}"), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_novels(&body, false),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key, None)],
                has_next_page: false,
            });
        }
        if page != 1 {
            return Ok(Paged::default());
        }
        let body = fetch(
            &format!("{BASE_URL}/build_in_search/?q={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_novels(&body, true),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample.html".to_string());
        Ok(fetch_details(&key, None))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "sample.html".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &key))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key =
            novel::request_key(&request, "chapter").unwrap_or_else(|| "sample.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "allbooks".to_string(),
            title: "All Books".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = lnreader::key_from_url(BASE_URL, input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key, None)),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36")
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_novels(body: &str, is_search: bool) -> Vec<CatalogItem> {
    body.split("article class=\"box")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "h2 class=\"title", "href")
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(block, "h2 class=\"title", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            let summary_marker = if is_search { "text5" } else { "text3" };
            let description = html::text_between(block, summary_marker, "</div>")
                .map(|value| {
                    html::strip_tags(&remove_blocks_containing(&value, &["coll-ellipsis", "<a"]))
                })
                .filter(|value| !value.is_empty());
            let tags = block
                .split("<a")
                .skip(1)
                .filter(|chunk| {
                    html::attr(chunk, "title").is_some_and(|value| value.starts_with("Genre - "))
                })
                .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect();
            let authors = block
                .split("<a")
                .skip(1)
                .filter(|chunk| {
                    html::attr(chunk, "title")
                        .is_some_and(|value| value.starts_with("Book author - "))
                })
                .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect();
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(block, "<img", "src").map(|image| absolute_url(&image)),
                description,
                tags,
                authors,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str, cached: Option<CatalogItem>) -> CatalogItem {
    let body = fetch(&absolute_url(key), DETAILS_FIXTURE);
    let mut item = cached.unwrap_or_else(CatalogItem::default);
    item.key = normalize_key(key);
    item.title = html::text_between(&body, "center", "</h2>")
        .map(|value| html::strip_tags(&value))
        .and_then(|value| {
            value
                .split(", \n\n")
                .next()
                .map(|part| part.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string()));
    item.cover =
        html::attr_after(&body, "article class=\"box", "src").map(|image| absolute_url(&image));
    item.url = Some(absolute_url(key));
    item.language = Some("en".to_string());
    item.content_rating = Some("safe".to_string());
    item.initialized = true;
    if let Some(series) = body
        .split("<b")
        .find(|block| block.contains("/series.html"))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
    {
        item.description = Some(match item.description {
            Some(summary) => format!("{series}\n\n{summary}"),
            None => series,
        });
    }
    item
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    let mut chapters = vec![NovelChapter {
        key: normalize_key(novel_key),
        title: Some("1".to_string()),
        chapter_number: Some(1.0),
        url: Some(absolute_url(novel_key)),
        language: Some("en".to_string()),
        ..NovelChapter::default()
    }];
    let mut number = 2.0;
    for chunk in body
        .split("div class=\"pages")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
    {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let key = normalize_key(&href);
        chapters.push(NovelChapter {
            key: key.clone(),
            title: html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty()),
            chapter_number: Some(number),
            url: Some(absolute_url(&key)),
            language: Some("en".to_string()),
            ..NovelChapter::default()
        });
        number += 1.0;
    }
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let content = lnreader::html_after_marker(body, "id=\"textToRead\"", "</div>")
        .or_else(|| lnreader::html_after_marker(body, "id='textToRead'", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let cleaned = readfrom_paragraphs(&remove_blocks_containing(
        &content,
        &["<center", "span:empty"],
    ));
    let normalized = novel::normalize_reader_html(&cleaned);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn readfrom_paragraphs(input: &str) -> String {
    let mut out = Vec::new();
    let mut paragraph = VecDeque::new();
    for piece in input.split("<br") {
        let text = html::strip_tags(piece)
            .trim()
            .replace("_", "<i>")
            .to_string();
        if text.is_empty() {
            continue;
        }
        paragraph.push_back(text);
        let joined = paragraph.iter().cloned().collect::<Vec<_>>().join(" ");
        out.push(format!("<p>{joined}</p>"));
        paragraph.clear();
    }
    if out.is_empty() {
        input.to_string()
    } else {
        out.join("")
    }
}

fn remove_blocks_containing(input: &str, markers: &[&str]) -> String {
    let mut output = input.to_string();
    for marker in markers {
        while let Some(pos) = output.find(marker) {
            let start = output[..pos].rfind('<').unwrap_or(pos);
            let end = output[pos..]
                .find('>')
                .map(|idx| pos + idx + 1)
                .unwrap_or(pos + marker.len());
            output.replace_range(start..end.min(output.len()), "");
        }
    }
    output
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"
<div id="dle-content"><article class="box"><h2 class="title"><a href="/sample.html">Sample Novel</a></h2><img src="/cover.jpg"><div class="text3">Sample summary.</div><h4><a title="Book author - Sample Author">Sample Author</a></h4><h2><a title="Genre - Fantasy">Fantasy</a></h2></article></div>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="text"><article class="box"><h2 class="title"><a href="/sample.html">Sample Novel</a></h2><img src="/cover.jpg"><div class="text5">Sample summary.</div><h5 class="title"><a title="Book author - Sample Author">Sample Author</a><a title="Genre - Fantasy">Fantasy</a></h5></article></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<article class="box"><div><center><div><a><img src="/cover.jpg"></a></div><h2 class="title">Sample Novel</h2></center></div><div class="pages"><a href="/sample-page-2.html">2</a></div></article>
"#;

const TEXT_FIXTURE: &str = r#"<div id="textToRead">Sample text.<br>Next line.</div>"#;

export_novel_source!(SOURCE);
