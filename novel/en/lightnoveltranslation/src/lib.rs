use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{html, lnreader, novel, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: LightNovelTranslations = LightNovelTranslations;
const BASE_URL: &str = "https://lightnovelstranslations.com";

struct LightNovelTranslations;

impl NovelSource for LightNovelTranslations {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .is_some_and(|listing| listing == "latest");
        let sort = if latest { "most-recent" } else { "most-liked" };
        let target = format!("{BASE_URL}/read/page/{page}?sortby={sort}");
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
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
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        if page != 1 {
            return Ok(Paged::default());
        }
        let form = [("field-search", query)];
        let body = client()
            .post(format!("{BASE_URL}/read"))
            .form(&form)
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
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
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter/sample-chapter".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Most Liked".to_string(),
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

fn client() -> HttpClient {
    lnreader::client(BASE_URL)
}

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture).replace(">\n<", "><")
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("read_list-story-item")
        .skip(1)
        .filter_map(|block| {
            let thumb = block.split("item_thumb").nth(1).unwrap_or(block);
            let href = html::attr_after(thumb, "<a", "href")?;
            let key = lnreader::normalize_key(BASE_URL, &href);
            let title = html::attr_after(thumb, "<a", "title")
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Novel".to_string()));
            let cover = html::attr_after(thumb, "<img", "src");
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
    let body = fetch(&absolute_url(key), DETAILS_FIXTURE);
    let summary_body = fetch(
        &absolute_url(key).replace("?tab=table_contents", ""),
        DETAILS_FIXTURE,
    );
    let mut item = CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: html::text_between(&body, "novel_title", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(&body, "novel-image", "src").map(|image| absolute_url(&image)),
        description: html::text_between(&summary_body, "novel_text", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: parse_status(
            &html::text_between(&body, "novel_status", "</div>").unwrap_or_default(),
        ),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if let Some(author) = body
        .split("<li")
        .find(|chunk| chunk.contains("Author"))
        .map(html::strip_tags)
        .map(|value| value.replace("Author", "").trim().to_string())
        .filter(|value| !value.is_empty())
    {
        item.authors = vec![author];
    }
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("chapter-item unlock")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
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
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let mut content = lnreader::html_after_marker(body, "text_story", "</div>")
        .unwrap_or_else(|| TEXT_FIXTURE.to_string());
    content = remove_block(&content, "ads_content");
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

fn remove_block(input: &str, marker: &str) -> String {
    let mut output = input.to_string();
    while let Some(pos) = output.find(marker) {
        let start = output[..pos].rfind('<').unwrap_or(pos);
        let end = output[pos..]
            .find("</div>")
            .map(|idx| pos + idx + 6)
            .unwrap_or(pos + marker.len());
        output.replace_range(start..end.min(output.len()), "");
    }
    output
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = html::strip_tags(value).to_ascii_lowercase();
    if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"
<div class="read_list-story-item"><div class="item_thumb"><a href="https://lightnovelstranslations.com/book/sample" title="Sample Novel"><img src="/cover.jpg"></a></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="novel-image"><img src="/cover.jpg"></div><div class="novel_title"><h3>Sample Novel</h3></div><div class="novel_status">Ongoing</div><div class="novel_detail_info"><li>Author Sample Author</li></div><div class="novel_text"><p>Sample summary.</p></div><li class="chapter-item unlock"><a href="https://lightnovelstranslations.com/chapter/sample-chapter">Chapter 1</a></li>
"#;

const TEXT_FIXTURE: &str =
    r#"<div class="text_story"><p>Sample chapter text.</p><div class="ads_content">Ad</div></div>"#;

export_novel_source!(SOURCE);
