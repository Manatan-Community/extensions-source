use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Shu69 = Shu69;
const BASE_URL: &str = "https://www.69shu.xyz";

struct Shu69;

impl NovelSource for Shu69 {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str);
        let target = if listing == Some("latest") {
            format!("{BASE_URL}/rank/lastupdate/{page}.html")
        } else {
            let sort = lnreader::filter_string(&request, "sort", "none");
            if sort == "none" {
                format!(
                    "{BASE_URL}/rank/{}/{page}.html",
                    lnreader::filter_string(&request, "rank", "allvisit")
                )
            } else {
                format!("{BASE_URL}/sort/{sort}/{page}.html")
            }
        };
        let entries = parse_listing(&fetch(&target, LIST_FIXTURE));
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
        let body = lnreader::client(BASE_URL)
            .post(&format!("{BASE_URL}/search"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("searchkey={}", url::query_escape(query)).into_bytes())
            .send_text()
            .unwrap_or_else(|_| LIST_FIXTURE.to_string());
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "book/1.htm".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "book/1.htm".to_string());
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
            novel::request_key(&request, "chapter").unwrap_or_else(|| "book/1/1.html".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        let raw = html::text_between(&body, "id=\"chaptercontent\"", "</div>")
            .or_else(|| html::text_between(&body, "id='chaptercontent'", "</div>"))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let cleaned = raw
            .split("<p")
            .filter_map(|part| text_between(part, ">", "</p>"))
            .filter(|line| !line.contains("69书吧"))
            .map(|line| format!("<p>{line}</p>"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(text_response(
            &key,
            if cleaned.is_empty() { &raw } else { &cleaned },
        ))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Rank", popular),
            section("latest", "Latest", latest),
        ])
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
    body.split("book-coverlist")
        .filter_map(|block| {
            let href = attr_after(block, "class=\"cover\"", "href")
                .or_else(|| attr_after(block, "<a", "href"))?;
            let title = text_between(block, "class=\"name\"", "</h4>")
                .or_else(|| attr_after(block, "<a", "title"))
                .unwrap_or_else(|| "69shu".to_string());
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
        title: text_between(body, "<h1", "</h1>").unwrap_or_else(|| "69shu".to_string()),
        cover: attr_after(body, "class=\"cover\"", "src")
            .or_else(|| attr_after(body, "<img", "src"))
            .map(|v| absolute_url(&v)),
        description: text_between(body, "id=\"bookIntro\"", "</"),
        authors: attr_after(body, "caption-bookinfo", "title")
            .into_iter()
            .collect(),
        status: status_from_text(body),
        url: Some(absolute_url(key)),
        language: Some("zh".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut source = String::new();
    if let Some(all_url) =
        attr_after(body, "class=\"all\"", "href").or_else(|| attr_after(body, "all", "href"))
    {
        let first = fetch(&absolute_url(&all_url), CHAPTERS_FIXTURE);
        source.push_str(&first);
    } else {
        source.push_str(body);
    }
    source
        .split("<dd")
        .enumerate()
        .filter_map(|(index, dd)| {
            let href = attr_after(dd, "<a", "href")?;
            let title =
                text_between(dd, "<a", "</a>").unwrap_or_else(|| format!("Chapter {}", index + 1));
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

fn status_from_text(body: &str) -> ItemStatus {
    if body.contains("连载") {
        ItemStatus::Ongoing
    } else if body.contains("完") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
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

const LIST_FIXTURE: &str = r#"<div class="book-coverlist"><a class="cover" href="/book/1.htm"><img src="/cover.jpg"></a><h4 class="name">Sample</h4></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div id="bookIntro">Summary</div><dd class="all"><a href="/book/1/">All</a></dd>"#;
const CHAPTERS_FIXTURE: &str =
    r#"<dl class="panel-chapterlist"><dd><a href="/book/1/1.html">Chapter 1</a></dd></dl>"#;
const TEXT_FIXTURE: &str = r#"<div id="chaptercontent"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
