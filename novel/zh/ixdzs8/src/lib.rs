use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Ixdzs8 = Ixdzs8;
const BASE_URL: &str = "https://ixdzs8.com";

struct Ixdzs8;

impl NovelSource for Ixdzs8 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let entries = parse_listing(&fetch(
            &format!("{BASE_URL}/hot/?page={page}"),
            LIST_FIXTURE,
        ));
        Ok(Paged {
            has_next_page: !entries.is_empty(),
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
        let entries = parse_listing(&fetch(
            &format!("{BASE_URL}/bsearch?q={}", url::query_escape(query)),
            LIST_FIXTURE,
        ));
        Ok(Paged {
            has_next_page: false,
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/1.html".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/1.html".to_string());
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "read/1/p1.html".to_string());
        let mut body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        if body.contains("challenge") {
            if let Some(token) = value_after(&body, "let token", "\"", "\"") {
                body = fetch(
                    &format!(
                        "{}?challenge={}",
                        absolute_url(&key),
                        url::query_escape(&token)
                    ),
                    TEXT_FIXTURE,
                );
            }
        }
        let raw = html::text_between(&body, "<article", "</article>")
            .or_else(|| html::text_between(&body, "<section", "</section>"))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        Ok(text_response(&key, &clean_reader(&raw)))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "hot".to_string(),
            title: "Hot".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: page.entries,
            has_more: page.has_next_page,
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
    body.split("li class=\"burl")
        .filter_map(|block| {
            let href = attr_after(block, "<a", "href").or_else(|| html::attr(block, "data-url"))?;
            let title = attr_after(block, "<a", "title")
                .or_else(|| text_between(block, "<h3", "</h3>"))
                .unwrap_or_else(|| "ixdzs8".to_string());
            Some(CatalogItem {
                key: normalize_key(&href),
                title,
                cover: attr_after(block, "<img", "src").map(|v| absolute_url(&v)),
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
        title: text_between(body, "<h1", "</h1>").unwrap_or_else(|| "ixdzs8".to_string()),
        cover: attr_after(body, "n-img", "src")
            .or_else(|| attr_after(body, "<img", "src"))
            .map(|v| absolute_url(&v)),
        description: html::text_between(body, "id=\"intro\"", "</p>").map(|v| html::strip_tags(&v)),
        authors: text_between(body, "class=\"bauthor\"", "</a>")
            .into_iter()
            .collect(),
        tags: body
            .split("<em")
            .filter_map(|part| text_between(part, "<a", "</a>"))
            .collect(),
        status: if body.contains("end") || body.contains("完") {
            ItemStatus::Completed
        } else if body.contains("lz") {
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
    let Some(bid) = html::attr_after(body, "id=\"bid\"", "value") else {
        return Vec::new();
    };
    let text = lnreader::client(BASE_URL)
        .post(&format!("{BASE_URL}/novel/clist/"))
        .form(&[("bid", bid.as_str())])
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    let root = serde_json::from_str::<Value>(&text)
        .or_else(|_| serde_json::from_str(CHAPTERS_FIXTURE))
        .unwrap_or(Value::Null);
    root.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, ch)| {
            if ch.get("ctype").and_then(Value::as_str).unwrap_or("0") != "0" {
                return None;
            }
            let order = ch
                .get("ordernum")
                .and_then(value_string)
                .unwrap_or_else(|| (index + 1).to_string());
            let key = format!("read/{bid}/p{order}.html");
            Some(NovelChapter {
                key: key.clone(),
                title: ch.get("title").and_then(Value::as_str).map(str::to_string),
                chapter_number: Some((index + 1) as f32),
                url: Some(absolute_url(&key)),
                language: Some("zh".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn clean_reader(raw: &str) -> String {
    raw.split("<p")
        .filter_map(|part| text_between(part, ">", "</p>"))
        .filter(|line| !line.contains("推薦本書") && !line.contains("javascript:"))
        .map(|line| format!("<p>{line}</p>"))
        .collect::<Vec<_>>()
        .join("\n")
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

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

fn value_after(input: &str, marker: &str, start: &str, end: &str) -> Option<String> {
    let idx = input.find(marker)?;
    let rest = &input[idx..];
    let s = rest.find(start)? + start.len();
    let e = rest[s..].find(end)?;
    Some(rest[s..s + e].to_string())
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

const LIST_FIXTURE: &str = r#"<li class="burl"><div class="l-info"><h3><a href="/book/1.html" title="Sample">Sample</a></h3></div><div class="l-img"><img src="/cover.jpg"></div></li>"#;
const DETAILS_FIXTURE: &str = r#"<div class="novel"><div class="n-text"><h1>Sample</h1><p><span class="lz">ongoing</span></p></div></div><input id="bid" value="1"><p id="intro">Summary</p>"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"rs":200,"data":[{"title":"Chapter 1","ctype":"0","ordernum":"1"}]}"#;
const TEXT_FIXTURE: &str = r#"<article><section><p>Sample text.</p></section></article>"#;

export_novel_source!(SOURCE);
