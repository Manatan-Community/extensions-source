use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Linovel = Linovel;
const BASE_URL: &str = "https://www.linovel.net";

struct Linovel;

impl NovelSource for Linovel {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let entries = parse_listing(
            &fetch(BASE_URL, LIST_FIXTURE),
            "book-item-inner",
            "book-item-name",
        );
        Ok(Paged {
            has_next_page: false,
            entries,
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
        if request.get("page").and_then(Value::as_u64).unwrap_or(1) > 1 {
            return Ok(Paged::default());
        }
        let body = fetch(
            &format!("{BASE_URL}/search/?kw={}", url::query_escape(query)),
            SEARCH_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body, "search-book", "book-name"),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/1.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/1.html".to_string());
        Ok(parse_chapters(&fetch(&absolute_url(&key), DETAILS_FIXTURE)))
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "novel/1/1.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        if body.contains("fufei-app-download-hint") {
            return Ok(text_response(
                &key,
                "<p>This chapter requires subscription in the source app.</p>",
            ));
        }
        let raw = html::text_between(&body, "article-text", "</div>")
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(text_response(&key, &parse_article(&raw)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "home".to_string(),
            title: "linovel".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: page.entries,
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

fn parse_listing(body: &str, item_marker: &str, title_marker: &str) -> Vec<CatalogItem> {
    body.split(item_marker)
        .filter_map(|block| {
            let href = html::attr(block, "href")?;
            let title =
                text_between(block, title_marker, "</").unwrap_or_else(|| "linovel".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: attr_after(block, "<img", "data-original")
                    .or_else(|| attr_after(block, "<img", "src"))
                    .map(|v| absolute_url(&v).replace("!min300jpg", "")),
                url: Some(absolute_url(&href)),
                language: Some("zh".to_string()),
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    parse_details(&fetch(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: text_between(body, "book-title", "</").unwrap_or_else(|| "linovel".to_string()),
        cover: attr_after(body, "book-cover", "src").map(|v| absolute_url(&v)),
        description: text_between(body, "about-text", "</"),
        authors: text_between(body, "div class=\"name\"", "</div>")
            .into_iter()
            .collect(),
        tags: body
            .split("book-cats")
            .skip(1)
            .flat_map(|part| {
                part.split("<a")
                    .skip(1)
                    .filter_map(|a| text_between(a, ">", "</a>"))
                    .collect::<Vec<_>>()
            })
            .collect(),
        status: if body.contains("已完结") {
            ItemStatus::Completed
        } else if body.contains("连载中") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(key)),
        language: Some("zh".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("div class=\"chapter")
        .enumerate()
        .filter_map(|(index, block)| {
            let href = attr_after(block, "<a", "href")?;
            let title = text_between(block, "<a", "</a>")
                .unwrap_or_else(|| format!("Chapter {}", index + 1));
            Some(NovelChapter {
                key: normalize_key(&href),
                title: Some(title),
                chapter_number: Some((index + 1) as f32),
                url: Some(absolute_url(&href)),
                language: Some("zh".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_article(raw: &str) -> String {
    let mut out = String::new();
    for part in raw.split("<p").skip(1) {
        if let Some(text) = text_between(part, ">", "</p>") {
            out.push_str("<p>");
            out.push_str(&text);
            out.push_str("</p>\n");
        }
    }
    for part in raw.split("<img").skip(1) {
        if let Some(src) = html::attr(part, "src") {
            out.push_str(&format!(r#"<img src="{}">"#, absolute_url(&src)));
        }
    }
    if out.trim().is_empty() {
        raw.to_string()
    } else {
        out
    }
}

fn text_response(key: &str, raw: &str) -> NovelText {
    let normalized = novel::normalize_reader_html(raw);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.8; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn absolute_url(key: &str) -> String {
    lnreader::absolute_url(BASE_URL, key)
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    lnreader::key_from_url(BASE_URL, input)
}

fn attr_after(input: &str, marker: &str, attr: &str) -> Option<String> {
    html::attr_after(input, marker, attr).filter(|value| !value.trim().is_empty())
}

fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    if start == ">" {
        let idx = input.find('>')?;
        let rest = &input[idx + 1..];
        let end_idx = rest.find(end)?;
        return Some(html::strip_tags(&rest[..end_idx]));
    }
    html::text_between(input, start, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

const LIST_FIXTURE: &str = r#"<a class="book-item-inner" href="/novel/1.html"><img data-original="/cover.jpg"><span class="book-item-name">Sample</span></a>"#;
const SEARCH_FIXTURE: &str = r#"<a class="search-book" href="/novel/1.html"><img src="/cover.jpg"><span class="book-name">Sample</span></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="book-title">Sample</div><div class="book-cover"><img src="/cover.jpg"></div><div class="about-text">Summary</div><div class="chapter"><a href="/novel/1/1.html">Chapter 1</a></div>"#;
const TEXT_FIXTURE: &str = r#"<div class="article-text"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
