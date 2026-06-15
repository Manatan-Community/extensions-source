use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{dates, html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: NovelHi = NovelHi;
const BASE_URL: &str = "https://novelhi.com";

struct NovelHi;

impl NovelSource for NovelHi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let json = lnreader::fetch_json(
            BASE_URL,
            &search_api_url(&request, None, page),
            LIST_FIXTURE,
        );
        let entries = parse_api_list(&json);
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
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key, None)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let json = lnreader::fetch_json(
            BASE_URL,
            &search_api_url(&request, Some(query), page),
            LIST_FIXTURE,
        );
        let entries = parse_api_list(&json);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "s/sample".to_string());
        Ok(fetch_details(&key, None))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "s/sample".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), DETAILS_FIXTURE);
        let book_id = html::attr_after(&body, "id=\"bookId\"", "value")
            .or_else(|| html::attr_after(&body, "id='bookId'", "value"));
        Ok(book_id
            .map(|id| fetch_chapters(&key, &id))
            .unwrap_or_default())
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "s/sample/1".to_string());
        let body = lnreader::fetch_document(BASE_URL, &absolute_url(&key), TEXT_FIXTURE);
        let content_path = html::attr_after(&body, "id=\"chapterContentPath\"", "value")
            .or_else(|| html::attr_after(&body, "id='chapterContentPath'", "value"));
        let token = html::attr_after(&body, "id=\"chapterContentToken\"", "value")
            .or_else(|| html::attr_after(&body, "id='chapterContentToken'", "value"));
        let content = if let (Some(path), Some(token)) = (content_path, token) {
            let content_url = format!(
                "{}?token={}",
                lnreader::absolute_url(BASE_URL, &path),
                url::query_escape(&token)
            );
            let json = lnreader::client(BASE_URL)
                .get(&content_url)
                .referer(&absolute_url(&key))
                .header("X-Requested-With", "XMLHttpRequest")
                .send_text()
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .unwrap_or_else(|| serde_json::from_str(CONTENT_FIXTURE).unwrap_or(Value::Null));
            json.pointer("/data/content")
                .and_then(Value::as_str)
                .map(rot13_html)
                .unwrap_or_default()
        } else {
            lnreader::html_after_marker(&body, "id=\"showReading\"", "</div>")
                .unwrap_or_else(|| TEXT_FIXTURE.to_string())
        };
        let cleaned = content
            .replace("<sent", "<p")
            .replace("</sent>", "</p>")
            .replace("<br/>", "")
            .replace("<br />", "");
        let normalized = novel::normalize_reader_html(&cleaned);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(absolute_url(&key)),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            ..NovelText::default()
        })
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "search".to_string(),
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

fn search_api_url(request: &Value, keyword: Option<&str>, page: u64) -> String {
    let mut params = vec![
        ("curr".to_string(), page.to_string()),
        ("limit".to_string(), "10".to_string()),
    ];
    if let Some(keyword) = keyword.filter(|value| !value.is_empty()) {
        params.push(("keyword".to_string(), keyword.to_string()));
    }
    if let Some(genre) = lnreader::filter_string_opt(request, "genres") {
        params.push(("bookGenres[]".to_string(), genre));
    }
    if let Some(status) = lnreader::filter_string_opt(request, "order") {
        params.push(("bookStatus".to_string(), status));
    }
    if let Some(time) = lnreader::filter_string_opt(request, "time") {
        params.push(("updatePeriod".to_string(), time));
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/book/searchByPageInShelf?{query}")
}

fn parse_api_list(json: &Value) -> Vec<CatalogItem> {
    json.pointer("/data/list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(api_item)
        .collect()
}

fn api_item(item: &Value) -> CatalogItem {
    let key = format!(
        "s/{}",
        item.get("simpleName")
            .and_then(Value::as_str)
            .unwrap_or("sample")
    );
    CatalogItem {
        key: key.clone(),
        title: item
            .get("bookName")
            .and_then(Value::as_str)
            .unwrap_or("Untitled")
            .to_string(),
        cover: item
            .get("picUrl")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        description: item
            .get("bookDesc")
            .and_then(Value::as_str)
            .map(|value| html::strip_tags(&value.replace("<br>", "\n"))),
        authors: item
            .get("authorName")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|genre| genre.get("genreName").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        status: if item.get("bookStatus").and_then(Value::as_str) == Some("1") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn fetch_details(key: &str, cached: Option<CatalogItem>) -> CatalogItem {
    let body = lnreader::fetch_document(BASE_URL, &absolute_url(key), DETAILS_FIXTURE);
    let mut item = cached.unwrap_or_else(CatalogItem::default);
    item.key = lnreader::normalize_key(BASE_URL, key);
    item.title = html::text_between(&body, "class=\"tit", "</h1>")
        .or_else(|| lnreader::text_between_tag(&body, "h1"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| item.title);
    item.cover = html::attr_after(&body, "class=\"cover", "src")
        .or_else(|| html::attr_after(&body, "decorate-img", "src"))
        .map(|image| absolute_url(&image))
        .or(item.cover);
    item.url = Some(absolute_url(key));
    item.language = Some("en".to_string());
    item.content_rating = Some("safe".to_string());
    item.initialized = true;
    item
}

fn fetch_chapters(novel_key: &str, book_id: &str) -> Vec<NovelChapter> {
    let api = format!(
        "{BASE_URL}/book/queryIndexList?bookId={}&curr=1&limit=42121",
        url::query_escape(book_id)
    );
    let json = lnreader::fetch_json(BASE_URL, &api, CHAPTERS_FIXTURE);
    let mut chapters: Vec<_> = json
        .pointer("/data/list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let index = chapter.get("indexNum").and_then(Value::as_str)?;
            let key = format!("{}/{}", novel_key.trim_end_matches('/'), index);
            Some(NovelChapter {
                key: key.clone(),
                title: chapter
                    .get("indexName")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                date_uploaded: chapter
                    .get("createTime")
                    .and_then(Value::as_str)
                    .and_then(dates::parse_fixture_date),
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    chapters.reverse();
    chapters
}

fn rot13_html(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            'a'..='z' => ((((ch as u8 - b'a') + 13) % 26) + b'a') as char,
            'A'..='Z' => ((((ch as u8 - b'A') + 13) % 26) + b'A') as char,
            _ => ch,
        })
        .collect()
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"{"data":{"list":[{"bookName":"Sample Novel","picUrl":"/cover.jpg","simpleName":"sample","authorName":"Sample Author","bookDesc":"Sample summary.","bookStatus":"0","genres":[{"genreName":"Fantasy"}]}]}}"#;
const DETAILS_FIXTURE: &str = r#"<div class="tit"><h1>Sample Novel</h1></div><img class="cover" src="/cover.jpg"><input id="bookId" value="1">"#;
const CHAPTERS_FIXTURE: &str =
    r#"{"data":{"list":[{"indexNum":"1","indexName":"Chapter 1","createTime":"2024-01-01"}]}}"#;
const CONTENT_FIXTURE: &str = r#"{"data":{"content":"<sent>Fnzcyr grkg.</sent>"}}"#;
const TEXT_FIXTURE: &str = r#"<input id="chapterContentPath" value="/content/sample"><input id="chapterContentToken" value="token"><div id="showReading"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
