use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LeafStudio = LeafStudio;
const BASE_URL: &str = "https://leafstudio.site";

struct LeafStudio;

impl NovelSource for LeafStudio {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if page > 1 {
            format!("{BASE_URL}/novels/page/{page}")
        } else {
            format!("{BASE_URL}/novels")
        };
        let body = lnreader::fetch_document(BASE_URL, &target, LIST_FIXTURE);
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
        let page_path = if page > 1 {
            format!("/page/{page}")
        } else {
            String::new()
        };
        let target = format!(
            "{BASE_URL}/novels{page_path}?search={}&type=&language=&status=&sort=",
            url::query_escape(query)
        );
        let body = lnreader::fetch_document(BASE_URL, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), DETAILS_FIXTURE);
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
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "novel/sample/chapter-1".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "novels".to_string(),
            title: "Novels".to_string(),
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

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("novel-item")
        .skip(1)
        .filter_map(|block| {
            let href =
                html::attr_after(block, "<a", "href").or_else(|| html::attr(block, "href"))?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = lnreader::text_after_marker(block, "novel-item-title", "</")
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            let cover = html::attr_after(block, "novel-item-Cover", "src")
                .or_else(|| html::attr_after(block, "<img", "src"));
            Some(CatalogItem {
                key: key.clone(),
                title,
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
    let body = lnreader::fetch_document(BASE_URL, &absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: lnreader::text_after_marker(&body, "h1 class=\"title", "</h1>")
            .or_else(|| lnreader::text_between_tag(&body, "h1"))
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&body, "id=\"novel_cover\"", "src")
            .or_else(|| html::attr_after(&body, "id='novel_cover'", "src"))
            .map(|image| absolute_url(&image)),
        description: description(&body),
        tags: body
            .split("novel_genre")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: status_from_text(
            &lnreader::text_after_marker(&body, "novel_status", "</").unwrap_or_default(),
        ),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn description(body: &str) -> Option<String> {
    let mut parts = Vec::new();
    for block in body.split("desc_div").skip(1) {
        for paragraph in block.split("<p").skip(1) {
            if let Some(text) = html::text_between(paragraph, ">", "</p>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
            {
                parts.push(text);
            }
        }
        break;
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut chapters: Vec<_> = body
        .split("free_chap chap")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, ">", "</a>")
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
        .collect();
    chapters.reverse();
    chapters
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let content = body
        .split("chapter_content")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</p>"))
        .collect::<Vec<_>>()
        .join("<br>");
    let html_body = if content.trim().is_empty() {
        TEXT_FIXTURE.to_string()
    } else {
        content
    };
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn status_from_text(value: &str) -> ItemStatus {
    match value.trim() {
        "Active" => ItemStatus::Ongoing,
        text if text.eq_ignore_ascii_case("completed") => ItemStatus::Completed,
        text if text.eq_ignore_ascii_case("hiatus") => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"
<a class="novel-item" href="https://leafstudio.site/novel/sample"><img class="novel-item-Cover" src="/cover.jpg"><p class="novel-item-title">Sample Novel</p></a>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="title">Sample Novel</h1><img id="novel_cover" src="/cover.jpg"><div class="desc_div"><p>Sample summary.</p></div><div id="tags_div"><a class="novel_genre">Fantasy</a></div><a id="novel_status">Active</a><a class="free_chap chap" href="https://leafstudio.site/novel/sample/chapter-1">Chapter 1</a>
"#;

const TEXT_FIXTURE: &str = r#"<article><p class="chapter_content">Sample text.</p></article>"#;

export_novel_source!(SOURCE);
