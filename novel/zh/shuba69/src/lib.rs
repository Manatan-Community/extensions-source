use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    browser::{
        self, WebViewRequest, WebViewRequestMethod, WebViewResponse, WebViewScript, WebViewSession,
        WebViewWait, WebViewWaitUntil,
    },
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, NovelChapter, NovelContentBlock, NovelText,
        OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "shuba69";
const BASE_URL: &str = "https://www.69shuba.com";
const CHALLENGE_TIMEOUT_MS: u64 = 45_000;

pub struct Shuba69Source;

impl Default for Shuba69Source {
    fn default() -> Self {
        Self
    }
}

impl Shuba69Source {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        self.document_request(url, WebViewRequestMethod::Get, None)
    }

    fn document_request(
        &self,
        url: &str,
        method: WebViewRequestMethod,
        body: Option<Vec<u8>>,
    ) -> Result<(Html, String)> {
        // 69书吧 challenges ordinary HTTP clients. The host-owned, source-scoped
        // browser handles that challenge on every platform and returns only a
        // sanitized DOM snapshot to the extension.
        let response: WebViewResponse = browser::open(&WebViewRequest {
            url: url.to_owned(),
            method,
            body,
            cookie_url: Some(BASE_URL.to_owned()),
            session: Some(WebViewSession {
                id: "69shuba-cloudflare".to_owned(),
                ..WebViewSession::default()
            }),
            wait_for: Some(WebViewWait::Script {
                script: r#"document.readyState === "complete" &&
                    document.title !== "Just a moment..." &&
                    !document.getElementById("challenge-error-title") &&
                    !document.querySelector('.cf-turnstile, [name="cf-turnstile-response"]') &&
                    !!document.querySelector(".container")"#
                    .to_owned(),
            }),
            wait_until: Some(WebViewWaitUntil::LoadFinished),
            headers: vec![
                (
                    "Accept".to_owned(),
                    "text/html,application/xhtml+xml".to_owned(),
                ),
                ("Accept-Language".to_owned(), "zh-CN,zh;q=0.9".to_owned()),
                ("Referer".to_owned(), format!("{BASE_URL}/")),
            ],
            timeout_ms: Some(CHALLENGE_TIMEOUT_MS),
            return_html: false,
            scripts: vec![WebViewScript {
                id: Some("69shuba-html".to_owned()),
                script: r#"(() => {
                    const root = document.querySelector(".container") || document.body;
                    const clone = root.cloneNode(true);
                    clone.querySelectorAll(
                        "script,style,noscript,iframe,object,embed,form,svg,.contentadv,.bottom-ad,#txtright"
                    ).forEach(node => node.remove());
                    return "<!doctype html><html><body>" + clone.outerHTML + "</body></html>";
                })()"#
                .to_owned(),
                run_at: None,
            }],
            ..WebViewRequest::default()
        })?;
        let rendered = response
            .script_results
            .iter()
            .find(|result| result.id.as_deref() == Some("69shuba-html"))
            .and_then(|result| result.value.as_ref())
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::new("69书吧 browser returned no readable HTML"))?;
        Ok((html::document(rendered), response.final_url))
    }

    fn browse(&self, page: u32, filters: &Value, fallback: &str) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let category = filters
            .get("category")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != "0");
        let url = category
            .map(|category| format!("{BASE_URL}/novels/class/{category}.htm"))
            .unwrap_or_else(|| fallback.to_owned());
        let (document, _) = self.document(&url)?;
        Self::parse_catalog(&document)
    }

    fn search_page(&self, query: &str, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 || query.trim().is_empty() {
            return Ok(Paged::new(Vec::new(), false));
        }
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("searchkey", query.trim())
            .append_pair("searchtype", "all")
            .finish()
            .into_bytes();
        let (document, _) = self.document_request(
            &format!("{BASE_URL}/modules/article/search.php"),
            WebViewRequestMethod::Post,
            Some(body),
        )?;
        Self::parse_catalog(&document)
    }

    fn parse_catalog(document: &Html) -> Result<Paged<CatalogItem>> {
        let rows = selector(".newbox li, #article_list_content > li")?;
        let title_links = selector(".newnav h3 a[href*='/book/']")?;
        let covers = selector("a.imgbox img")?;
        let descriptions = selector(".newnav ol")?;
        let labels = selector(".labelbox label")?;
        let mut entries = Vec::new();
        for row in document.select(&rows) {
            let Some(link) = row
                .select(&title_links)
                .find(|link| !normalize_space(&html::text(*link)).is_empty())
            else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(link));
            if title.is_empty() {
                continue;
            }
            let page_url = canonical_item_url(&absolute_url(BASE_URL, &href)?)?;
            let metadata = row
                .select(&labels)
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            let mut item = CatalogItem::new(page_url.clone(), title);
            item.url = Some(page_url.clone());
            item.cover = row
                .select(&covers)
                .next()
                .and_then(|node| ["data-src", "src"].iter().find_map(|name| attr(node, name)))
                .filter(|value| !value.contains("/images/nocover."))
                .map(|cover| absolute_url(BASE_URL, &cover))
                .transpose()?
                .map(|cover| image(&cover, &page_url));
            item.description = row
                .select(&descriptions)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            if let Some(author) = metadata.first() {
                item.authors.push(author.clone());
            }
            if metadata.len() > 2 {
                item.tags.push(metadata[metadata.len() - 2].clone());
            }
            item.status = metadata.last().map(|value| json!(normalize_status(value)));
            item.language = Some("zh".into());
            item.content_rating = Some(content_rating(&item.tags).into());
            entries.push(item);
        }
        Ok(Paged::new(entries, false))
    }

    fn parse_latest(document: &Html) -> Result<Paged<CatalogItem>> {
        let rows = selector(".recentupdate2 li")?;
        let links = selector("a[href*='/book/']")?;
        let mut entries = Vec::new();
        for row in document.select(&rows) {
            let Some(link) = row.select(&links).next() else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(link));
            if title.is_empty() {
                continue;
            }
            let page_url = canonical_item_url(&absolute_url(BASE_URL, &href)?)?;
            let mut item = CatalogItem::new(page_url.clone(), title);
            item.url = Some(page_url);
            item.language = Some("zh".into());
            item.content_rating = Some("suggestive".into());
            entries.push(item);
        }
        Ok(Paged::new(entries, false))
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = first_text(document, ".booknav2 h1")?
            .ok_or_else(|| Error::new("69书吧 novel has no title"))?;
        let tags = texts(document, "#tagul a")?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.into());
        item.cover = first_attr(document, ".bookimg2 img", "src")?
            .map(|cover| absolute_url(BASE_URL, &cover))
            .transpose()?
            .map(|cover| image(&cover, page_url));
        item.description = first_text(document, ".navtxt p:first-child")?;
        item.authors = texts(document, ".booknav2 a[href*='author.php']")?;
        item.tags = tags;
        if let Some(category) = first_text(document, ".booknav2 a[href*='/novels/class/']")? {
            if !item.tags.contains(&category) {
                item.tags.insert(0, category);
            }
        }
        item.status =
            first_text(document, ".booknav2")?.map(|value| json!(normalize_status(&value)));
        item.language = Some("zh".into());
        item.content_rating = Some(content_rating(&item.tags).into());
        item.initialized = true;
        Ok(item)
    }

    fn parse_chapters(document: &Html) -> Result<Vec<NovelChapter>> {
        let links = selector("#catalog a[href*='/txt/']")?;
        let number_re =
            Regex::new(r"(?:第\s*)?(\d+(?:\.\d+)?)\s*章").map_err(|e| Error::new(e.to_string()))?;
        let mut chapters = Vec::new();
        for link in document.select(&links) {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let chapter_url = absolute_url(BASE_URL, &href)?;
            let value = normalize_space(&html::text(link));
            let title = (!value.is_empty()).then_some(value);
            let chapter_number = title
                .as_deref()
                .and_then(|title| number_re.captures(title))
                .and_then(|captures| captures.get(1))
                .and_then(|value| value.as_str().parse().ok());
            chapters.push(NovelChapter {
                key: chapter_url.clone(),
                title,
                chapter_number,
                url: Some(chapter_url),
                language: Some("zh".into()),
                ..NovelChapter::default()
            });
        }
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter.source_order = Some(index as i32);
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "69书吧 novel has no chapters",
        )?;
        Ok(chapters)
    }

    fn parse_text(document: &Html, chapter_url: &str) -> Result<NovelText> {
        let title = first_text(document, ".txtnav h1")?;
        let raw = first_inner_html(document, ".txtnav")?
            .ok_or_else(|| Error::new("69书吧 chapter has no readable content"))?;
        let rendered = chapter_paragraphs(&raw, title.as_deref())?;
        require(
            (!rendered.is_empty()).then_some(()),
            "69书吧 chapter has no readable content",
        )?;
        Ok(NovelText {
            html: Some(rendered.clone()),
            title,
            base_url: Some(chapter_url.into()),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        canonical_item_url(item.url.as_deref().unwrap_or(&item.key))
    }
}

impl NovelSource for Shuba69Source {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, &json!({}), &format!("{BASE_URL}/novels/hot"))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let (document, _) = self.document(&format!("{BASE_URL}/last.html"))?;
        Self::parse_latest(&document)
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.browse(page, filters, &format!("{BASE_URL}/novels/hot")),
            "latest"
                if filters
                    .get("category")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty() && value != "0") =>
            {
                self.browse(page, filters, &format!("{BASE_URL}/novels/class/0.htm"))
            }
            "latest" => self.latest(page),
            _ => Err(Error::new(format!("unknown 69书吧 listing {listing:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        self.search_page(query, page)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &canonical_item_url(&final_url)?)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = Self::item_url(&item)?;
        let id = book_id(&url)?;
        let (document, _) = self.document(&format!("{BASE_URL}/book/{id}/"))?;
        Self::parse_chapters(&document)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = absolute_url(BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_text(&document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "category".into(),
            name: "分类".into(),
            options: CATEGORIES
                .iter()
                .map(|(label, value)| OptionItem {
                    label: (*label).into(),
                    value: (*value).into(),
                })
                .collect(),
            default_index: 0,
        }])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if !matches!(url.host_str(), Some("69shuba.com" | "www.69shuba.com")) {
            return Ok(None);
        }
        let book_re = Regex::new(r"^/book/(\d+)(?:\.htm|/)?$").unwrap();
        if let Some(captures) = book_re.captures(url.path()) {
            let item_url = format!("{BASE_URL}/book/{}.htm", &captures[1]);
            let mut item = CatalogItem::new(item_url.clone(), "");
            item.url = Some(item_url);
            item.language = Some("zh".into());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        let chapter_re = Regex::new(r"^/txt/(\d+)/(\d+)/?$").unwrap();
        let Some(captures) = chapter_re.captures(url.path()) else {
            return Ok(None);
        };
        let item_url = format!("{BASE_URL}/book/{}.htm", &captures[1]);
        let chapter_url = format!("{BASE_URL}/txt/{}/{}", &captures[1], &captures[2]);
        let mut item = CatalogItem::new(item_url.clone(), "");
        item.url = Some(item_url);
        item.language = Some("zh".into());
        let chapter = NovelChapter {
            key: chapter_url.clone(),
            url: Some(chapter_url),
            language: Some("zh".into()),
            ..NovelChapter::default()
        };
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter: Some(chapter),
            ..UrlResolveResult::default()
        }))
    }
}

fn canonical_item_url(candidate: &str) -> Result<String> {
    let absolute = absolute_url(BASE_URL, candidate)?;
    let id = book_id(&absolute)?;
    Ok(format!("{BASE_URL}/book/{id}.htm"))
}

fn book_id(candidate: &str) -> Result<String> {
    let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
    let re = Regex::new(r"^/book/(\d+)(?:\.htm|/)?$").unwrap();
    re.captures(url.path())
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| Error::new("69书吧 item URL has no book id"))
}

fn first_text(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn texts(document: &Html, query: &str) -> Result<Vec<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty())
        .collect())
}

fn first_attr(document: &Html, query: &str, name: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .find_map(|element| attr(element, name)))
}

fn first_inner_html(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document.select(&query).next().map(|node| node.inner_html()))
}

fn chapter_paragraphs(value: &str, title: Option<&str>) -> Result<String> {
    let unwanted = Regex::new(
        r#"(?is)<h1\b[^>]*>.*?</h1\s*>|<div\b[^>]*(?:id\s*=\s*["']txtright["']|class\s*=\s*["'][^"']*(?:txtinfo|contentadv|bottom-ad)[^"']*["'])[^>]*>.*?</div\s*>|<(?:script|style|iframe|object|embed)\b[^>]*>.*?</(?:script|style|iframe|object|embed)\s*>"#,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    let mut cleaned = unwanted.replace_all(value, "").into_owned();
    if let Some(title) = title {
        let repeated_title = Regex::new(&format!(r"(?is)^\s*{}\s*<br\s*/?>", regex::escape(title)))
            .map_err(|error| Error::new(error.to_string()))?;
        cleaned = repeated_title.replace(&cleaned, "").into_owned();
    }
    let breaks =
        Regex::new(r"(?i)(?:<br\s*/?>\s*){2,}").map_err(|error| Error::new(error.to_string()))?;
    let mut paragraphs = Vec::new();
    for part in breaks.split(&cleaned) {
        let fragment = html::fragment(part);
        let text = normalize_space(&html::text(fragment.root_element()));
        if text.is_empty()
            || title.is_some_and(|title| text == normalize_space(title))
            || text.contains("69书吧")
        {
            continue;
        }
        paragraphs.push(format!("<p>{}</p>", escape_html(&text)));
    }
    Ok(paragraphs.join("\n"))
}

fn normalize_status(value: &str) -> &'static str {
    if value.contains("完本") || value.contains("完结") || value.contains("全本") {
        "completed"
    } else if value.contains("连载") {
        "ongoing"
    } else {
        "unknown"
    }
}

fn content_rating(tags: &[String]) -> &'static str {
    if tags.iter().any(|tag| {
        let value = tag.trim().to_ascii_lowercase();
        matches!(value.as_str(), "adult" | "mature" | "smut" | "ecchi")
            || matches!(tag.trim(), "成人" | "情色" | "肉文" | "限制级")
    }) {
        "adult"
    } else {
        "suggestive"
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url).header("Referer", referer)
}

const CATEGORIES: &[(&str, &str)] = &[
    ("全部分类", "0"),
    ("言情小说", "3"),
    ("玄幻魔法", "1"),
    ("修真武侠", "2"),
    ("穿越时空", "11"),
    ("都市小说", "9"),
    ("历史军事", "4"),
    ("游戏竞技", "5"),
    ("科幻空间", "6"),
    ("悬疑惊悚", "7"),
    ("同人小说", "8"),
    ("官场职场", "10"),
    ("青春校园", "12"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, Shuba69Source)
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_cover_metadata_and_status() {
        let document = html::document(include_str!("../tests/fixtures/catalog.html"));
        let page = Shuba69Source::parse_catalog(&document).unwrap();
        assert_eq!(page.entries.len(), 1);
        let item = &page.entries[0];
        assert_eq!(item.title, "测试小说");
        assert_eq!(item.authors, vec!["测试作者"]);
        assert_eq!(item.tags, vec!["玄幻魔法"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert!(item.cover.as_ref().unwrap().url.ends_with("/12345s.jpg"));
    }

    #[test]
    fn parses_details_tags_description_and_completion() {
        let document = html::document(include_str!("../tests/fixtures/details.html"));
        let item =
            Shuba69Source::parse_details(&document, &format!("{BASE_URL}/book/12345.htm")).unwrap();
        assert_eq!(item.title, "测试小说");
        assert_eq!(item.authors, vec!["测试作者"]);
        assert_eq!(item.status, Some(json!("completed")));
        assert_eq!(item.tags, vec!["玄幻魔法", "穿越", "系统流"]);
        assert_eq!(item.description.as_deref(), Some("第一段。 第二段。"));
        assert!(item.initialized);
    }

    #[test]
    fn parses_chapters_in_reading_order() {
        let document = html::document(include_str!("../tests/fixtures/chapters.html"));
        let chapters = Shuba69Source::parse_chapters(&document).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("第1章 开始"));
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        assert_eq!(chapters[1].source_order, Some(1));
    }

    #[test]
    fn sanitizes_chapter_text_and_ads() {
        let document = html::document(include_str!("../tests/fixtures/chapter.html"));
        let text =
            Shuba69Source::parse_text(&document, &format!("{BASE_URL}/txt/12345/2")).unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("<p>第一段 &amp; 内容。</p>"));
        assert!(rendered.contains("<p>第二段。</p>"));
        assert!(!rendered.contains("广告"));
        assert!(!rendered.contains("<script"));
    }

    #[test]
    fn resolves_book_and_chapter_urls() {
        let mut source = Shuba69Source;
        let item = source
            .handle_url("https://69shuba.com/book/12345/")
            .unwrap()
            .unwrap();
        assert_eq!(
            item.item.unwrap().url.as_deref(),
            Some("https://www.69shuba.com/book/12345.htm")
        );
        let chapter = source
            .handle_url("https://www.69shuba.com/txt/12345/67890")
            .unwrap()
            .unwrap();
        assert!(chapter.novel_chapter.is_some());
        assert_eq!(
            chapter.item.unwrap().url.as_deref(),
            Some("https://www.69shuba.com/book/12345.htm")
        );
    }
}
