use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: FaqWikiUs = FaqWikiUs;
const BASE_URL: &str = "https://faqwiki.us/novel";

struct FaqWikiUs;

impl NovelSource for FaqWikiUs {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let body = fetch(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_novels(&body, None),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let body = fetch(BASE_URL, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_novels(&body, Some(query)),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "/sample".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &fetch_details(&key).title))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapter".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "novels".to_string(),
            title: "Novels".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn parse_novels(body: &str, search: Option<&str>) -> Vec<CatalogItem> {
    let query = search.unwrap_or_default().to_ascii_lowercase();
    body.split("plt-page-item")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::strip_tags(block)
                .replace("Novel - All Chapters", "")
                .replace("Novel – All Chapters", "")
                .trim()
                .to_string();
            if !query.is_empty() && !title.to_ascii_lowercase().contains(&query) {
                return None;
            }
            let cover = html::attr_after(block, "<img", "data-ezsrc")
                .or_else(|| html::attr_after(block, "<img", "src"))
                .map(|value| {
                    value
                        .split("?ezimgfmt=")
                        .next()
                        .unwrap_or(&value)
                        .to_string()
                });
            Some(CatalogItem {
                key: key.clone(),
                title: if title.is_empty() {
                    url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string())
                } else {
                    title
                },
                cover: cover.map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch(&absolute_url(key), DETAILS_FIXTURE);
    let mut item = CatalogItem {
        key: normalize_key(key),
        title: lnreader::text_after_marker(&body, "entry-title", "</")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string()))
            .replace("Novel - All Chapters", "")
            .replace("Novel – All Chapters", "")
            .trim()
            .to_string(),
        cover: html::attr_after(&body, "wp-block-image", "data-ezsrc")
            .or_else(|| html::attr_after(&body, "wp-block-image", "src"))
            .map(|value| {
                value
                    .split("?ezimgfmt=")
                    .next()
                    .unwrap_or(&value)
                    .to_string()
            })
            .map(|image| absolute_url(&image)),
        status: if body.to_ascii_lowercase().contains("complete") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    for block in body.split("<strong").skip(1) {
        let label = html::text_between(block, ">", "</strong>")
            .map(|value| html::strip_tags(&value).to_ascii_lowercase())
            .unwrap_or_default();
        let values = html::text_between(block, "</strong>", "</p>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_default();
        match label.trim() {
            "description:" => item.description = Some(values),
            "author(s):" => item.authors = vec![values],
            "genre:" => item.tags = split_genres(&values),
            _ => {}
        }
    }
    item
}

fn parse_chapters(body: &str, novel_name: &str) -> Vec<NovelChapter> {
    body.split("lcp_catlist")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .map(|value| {
                    value
                        .replace(novel_name, "")
                        .replace("Novel", "")
                        .trim()
                        .to_string()
                })
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                chapter_number: Some(index as f32 + 1.0),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let mut content = lnreader::html_after_marker(body, "entry-content", "</div>")
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    content = remove_blocks(&content, &["<script", "<span", "<div"]);
    let normalized = novel::normalize_reader_html(&content);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn split_genres(value: &str) -> Vec<String> {
    let normalized = value
        .replace("Slice of Life", "Slice_of_Life")
        .replace("School Life", "School_Life");
    normalized
        .split_whitespace()
        .map(|word| word.replace('_', " "))
        .filter(|value| !value.is_empty())
        .collect()
}

fn remove_blocks(input: &str, markers: &[&str]) -> String {
    let mut output = input.to_string();
    for marker in markers {
        while let Some(start) = output.find(marker) {
            let tag = marker.trim_start_matches('<');
            let close = format!("</{}>", tag.split_whitespace().next().unwrap_or(tag));
            let end = output[start..]
                .find(&close)
                .map(|idx| start + idx + close.len())
                .unwrap_or(start + marker.len());
            output.replace_range(start..end.min(output.len()), "");
        }
    }
    output
}

fn key_from_url(input: &str) -> Option<String> {
    input.contains("faqwiki.us").then(|| normalize_key(input))
}

fn normalize_key(input: &str) -> String {
    input
        .replace("tp:", "tps:")
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://faqwiki.us/novel")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        format!(
            "{}/{}",
            BASE_URL.trim_end_matches('/'),
            input.trim_start_matches('/')
        )
    }
}

const LIST_FIXTURE: &str = r#"
<div class="plt-page-item"><a href="https://faqwiki.us/novel/sample"><img data-ezsrc="/cover.jpg">Sample Novel – All Chapters</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="entry-title">Sample Novel – All Chapters</h1><figure class="wp-block-image"><img data-ezsrc="/cover.jpg?ezimgfmt=rs:1"></figure><div class="entry-content"><p><strong>Description:</strong>Sample summary.</p><p><strong>Author(s):</strong>Sample Author</p><p><strong>Genre:</strong>Fantasy Slice of Life</p><ul class="lcp_catlist"><li><a href="https://faqwiki.us/novel/sample-chapter">Sample Novel Chapter 1</a></li></ul></div>
"#;

const TEXT_FIXTURE: &str = r#"<div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
