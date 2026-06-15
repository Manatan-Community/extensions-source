use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Quanben = Quanben;
const BASE_URL: &str = "https://www.quanben.io";

struct Quanben;

impl NovelSource for Quanben {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let genre = lnreader::filter_string(&request, "genre", "all");
        let target = if genre == "all" {
            format!("{BASE_URL}/")
        } else {
            format!("{BASE_URL}/c/{genre}.html")
        };
        let entries = parse_listing(&fetch(&target, LIST_FIXTURE));
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
            &format!(
                "{BASE_URL}/index.php?c=book&a=search&keywords={}",
                url::query_escape(query)
            ),
            LIST_FIXTURE,
        );
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "n/sample/".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "n/sample/".to_string());
        Ok(parse_chapters(&key))
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "sample/1.html".to_string());
        let body = fetch(&format!("{BASE_URL}/n/{key}"), TEXT_FIXTURE);
        let raw = html::text_between(&body, "id=\"contentbody\"", "</div>")
            .or_else(|| html::text_between(&body, "id=\"content\"", "</div>"))
            .or_else(|| html::text_between(&body, "class=\"content\"", "</div>"))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(text_response(&key, &raw))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "home".to_string(),
            title: "Quanben".to_string(),
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    let mut out = Vec::new();
    for block in body.split("list2").skip(1) {
        let href = attr_after(block, "<a", "href");
        let title =
            text_between(block, "<h3", "</h3>").or_else(|| text_between(block, "<a", "</a>"));
        if let (Some(href), Some(title)) = (href, title) {
            if let Some(key) = standard_path(&href) {
                out.push(CatalogItem {
                    key: key.clone(),
                    title,
                    cover: attr_after(block, "<img", "src")
                        .or_else(|| attr_after(block, "<img", "data-src"))
                        .map(|v| absolute_url(&v)),
                    url: Some(absolute_url(&key)),
                    language: Some("zh".to_string()),
                    ..CatalogItem::default()
                });
            }
        }
    }
    out
}

fn fetch_details(key: &str) -> CatalogItem {
    parse_details(&fetch(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: meta(body, "og:novel:book_name")
            .or_else(|| text_between(body, "<h3", "</h3>"))
            .unwrap_or_else(|| "Quanben".to_string()),
        cover: meta(body, "og:image")
            .or_else(|| attr_after(body, "<img", "src").map(|v| absolute_url(&v))),
        description: meta(body, "og:description").or_else(|| {
            html::text_between(body, "description", "</div>").map(|v| html::strip_tags(&v))
        }),
        authors: meta(body, "og:novel:author").into_iter().collect(),
        tags: meta(body, "og:novel:category").into_iter().collect(),
        status: meta(body, "og:novel:status")
            .map(|s| {
                if s.contains("完") {
                    ItemStatus::Completed
                } else {
                    ItemStatus::Ongoing
                }
            })
            .unwrap_or(ItemStatus::Unknown),
        url: Some(absolute_url(key)),
        language: Some("zh".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(novel_key: &str) -> Vec<NovelChapter> {
    let slug = novel_key
        .trim_start_matches("n/")
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("sample");
    let target = format!("https://quanben5.com/n/{slug}/xiaoshuo.html");
    let body = lnreader::fetch_document(BASE_URL, &target, CHAPTERS_FIXTURE);
    body.split("<a")
        .enumerate()
        .filter_map(|(index, block)| {
            let title = text_between(block, ">", "</a>")?;
            if title.is_empty() {
                return None;
            }
            let key = format!("{slug}/{}.html", index + 1);
            Some(NovelChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: Some((index + 1) as f32),
                url: Some(format!("{BASE_URL}/n/{key}")),
                language: Some("zh".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn text_response(key: &str, raw: &str) -> NovelText {
    let normalized = novel::normalize_reader_html(raw);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/n/{key}")),
        css: Some("body { line-height: 1.8; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn meta(body: &str, prop: &str) -> Option<String> {
    let marker = format!("property=\"{prop}\"");
    html::attr_after(body, &marker, "content").filter(|value| !value.trim().is_empty())
}

fn standard_path(href: &str) -> Option<String> {
    let key = normalize_key(href).trim_start_matches("amp/").to_string();
    if key.starts_with("n/") && key.ends_with('/') {
        Some(key)
    } else {
        key.find("n/")
            .map(|idx| key[idx..].to_string())
            .filter(|v| v.ends_with('/'))
    }
}

fn absolute_url(key: &str) -> String {
    lnreader::absolute_url(BASE_URL, key)
}

fn normalize_key(input: &str) -> String {
    lnreader::normalize_key(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    lnreader::key_from_url(BASE_URL, input).and_then(|key| standard_path(&key))
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

const LIST_FIXTURE: &str =
    r#"<div class="list2"><h3><a href="/n/sample/">Sample</a></h3><img src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<meta property="og:novel:book_name" content="Sample"><meta property="og:description" content="Summary"><meta property="og:novel:status" content="连载"><div class="list2"><h3>Sample</h3></div>"#;
const CHAPTERS_FIXTURE: &str = r#"<ul><li><a href="1.html">Chapter 1</a></li></ul>"#;
const TEXT_FIXTURE: &str = r#"<div id="contentbody"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
