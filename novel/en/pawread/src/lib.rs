use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: PawRead = PawRead;
const BASE_URL: &str = "https://m.pawread.com";

struct PawRead;

impl NovelSource for PawRead {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = list_url(&request, page);
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{BASE_URL}/search/?keywords={}&page={page}",
            url::query_escape(query)
        );
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "book/sample".to_string());
        let body = fetch(
            &format!("{}/", absolute_url(&key).trim_end_matches('/')),
            DETAILS_FIXTURE,
        );
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
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "book/sample/1.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "list".to_string(),
            title: "List".to_string(),
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

fn list_url(request: &Value, page: u64) -> String {
    let values = ["genre", "status", "lang"]
        .into_iter()
        .filter_map(|key| lnreader::filter_string_opt(request, key))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let mut path = String::from("list/");
    if !values.is_empty() {
        path.push_str(&values.join("-"));
        path.push('/');
    }
    let sort = lnreader::filter_string(request, "sort", "click");
    let asc = lnreader::filter_bool(request, "order", false);
    if asc {
        path.push('-');
    }
    path.push_str(&sort);
    format!("{BASE_URL}/{path}/?page={page}")
}

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("<")
        .filter(|chunk| chunk.contains("list-comic") || chunk.contains("itemBox"))
        .filter_map(|block| {
            let href = html::attr_after(block, "txtA", "href")
                .or_else(|| html::attr_after(block, "title", "href"))
                .or_else(|| html::attr_after(block, "<a", "href"))?;
            let key = lnreader::normalize_key(BASE_URL, &href)
                .split('/')
                .take(2)
                .collect::<Vec<_>>()
                .join("/");
            let title = html::text_between(block, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(block, "<img", "src").map(|image| absolute_url(&image)),
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
    let body = fetch(
        &format!("{}/", absolute_url(key).trim_end_matches('/')),
        DETAILS_FIXTURE,
    );
    CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: html::attr_after(&body, "id=\"Cover\"", "title")
            .or_else(|| html::attr_after(&body, "id='Cover'", "title"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&body, "id=\"Cover\"", "src")
            .or_else(|| html::attr_after(&body, "id='Cover'", "src"))
            .map(|image| absolute_url(&image)),
        description: html::text_between(&body, "id=\"full-des\"", "</p>")
            .or_else(|| html::text_between(&body, "id='full-des'", "</p>"))
            .map(|value| html::strip_tags(&value)),
        tags: body
            .split("btn-default")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        authors: body
            .split("txtItme")
            .nth(2)
            .map(html::strip_tags)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        status: parse_status(
            &body
                .split("txtItme")
                .nth(1)
                .map(html::strip_tags)
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, novel_key: &str) -> Vec<NovelChapter> {
    body.split("item-box")
        .skip(1)
        .filter_map(|block| {
            let id = html::attr(block, "onclick").and_then(|onclick| {
                onclick
                    .split(|ch: char| !ch.is_ascii_digit())
                    .find(|part| !part.is_empty())
                    .map(ToString::to_string)
            })?;
            let key = format!("{}/{}.html", novel_key.trim_end_matches('/'), id);
            let title = html::text_between(block, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some(NovelChapter {
                key: key.clone(),
                title,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let mut content = lnreader::html_after_marker(body, "class=\"main\"", "</div>")
        .or_else(|| lnreader::html_after_marker(body, "class='main'", "</div>"))
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    for marker in ["pawread", "tinyurl", "bit.ly"] {
        content = remove_line_containing(&content, marker);
    }
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

fn remove_line_containing(input: &str, marker: &str) -> String {
    input
        .lines()
        .filter(|line| !line.contains(marker))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("completed") || lower.contains("wanjie") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Ongoing
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"<div class="list-comic"><a class="txtA" href="/book/sample"><img src="/cover.jpg">Sample Novel</a></div>"#;
const DETAILS_FIXTURE: &str = r#"
<div id="Cover"><img title="Sample Novel" src="/cover.jpg"></div><p class="txtItme">Ongoing</p><p class="txtItme">Sample Author</p><p id="full-des">Sample summary.</p><a class="btn-default">Fantasy</a><div class="item-box" onclick="read(1)"><span>2024.01.01</span><span>Chapter 1</span></div>
"#;
const TEXT_FIXTURE: &str = r#"<div class="main"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
