use std::collections::BTreeSet;

use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, ElementRef, Html},
    model::{
        CatalogItem, FilterDefinition, NovelChapter, NovelContentBlock, NovelText, OptionItem,
        Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "shuku52";
const BASE_URL: &str = "https://www.52shuku.net";

pub struct Shuku52Source {
    client: Client,
}

impl Default for Shuku52Source {
    fn default() -> Self {
        Self {
            client: Client::browser(),
        }
    }
}

#[derive(Clone, Copy)]
struct Category {
    label: &'static str,
    value: &'static str,
    latest_path: &'static str,
    popular_path: &'static str,
}

const CATEGORIES: &[Category] = &[
    Category {
        label: "现代耽美",
        value: "modern_bl",
        latest_path: "xiandaidushi",
        popular_path: "tuijian/xiandai_top100.html",
    },
    Category {
        label: "穿越耽美",
        value: "reincarnation_bl",
        latest_path: "chongsheng",
        popular_path: "tuijian/chongsheng_top100.html",
    },
    Category {
        label: "古架耽美",
        value: "ancient_bl",
        latest_path: "jiakong",
        popular_path: "tuijian/gudai_top100.html",
    },
    Category {
        label: "GL百合",
        value: "gl",
        latest_path: "gl",
        popular_path: "tuijian/gl_top100.html",
    },
    Category {
        label: "综漫同人",
        value: "fanfiction",
        latest_path: "bl",
        popular_path: "tuijian/bl_top100.html",
    },
    Category {
        label: "言情小说",
        value: "romance",
        latest_path: "yanqing",
        popular_path: "tuijian/yanqing_top100.html",
    },
    Category {
        label: "男频小说",
        value: "male",
        latest_path: "nan",
        popular_path: "nan/TuiJian.html",
    },
    Category {
        label: "武侠仙侠",
        value: "wuxia",
        latest_path: "wuxia",
        popular_path: "wuxia",
    },
    Category {
        label: "科幻空间",
        value: "science_fiction",
        latest_path: "KeHuan",
        popular_path: "KeHuan",
    },
];

impl Shuku52Source {
    fn document(&self, candidate: &str) -> Result<(Html, String)> {
        site_url(candidate)?;
        let response = self.client.get(candidate).send()?.error_for_status()?;
        let final_url = site_url(response.final_url())?.to_string();
        Ok((html::document(response.text()?), final_url))
    }

    fn category(filters: &Value) -> &'static Category {
        let value = filters.get("category").and_then(Value::as_str);
        value
            .and_then(|value| CATEGORIES.iter().find(|category| category.value == value))
            .unwrap_or(&CATEGORIES[0])
    }

    fn listing_url(category: &Category, page: u32, popular: bool) -> String {
        let page = page.max(1);
        let path = if popular {
            category.popular_path
        } else {
            category.latest_path
        };
        if popular || path.ends_with(".html") || page == 1 {
            format!("{BASE_URL}/{path}/").replace(".html/", ".html")
        } else {
            format!("{BASE_URL}/{path}/index_{page}.html")
        }
    }

    fn search_url(query: &str, page: u32) -> Result<String> {
        let mut url = Url::parse(&format!("{BASE_URL}/so/search.php"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", query.trim())
            .append_pair("m", "no")
            .append_pair("f", "_all")
            .append_pair("syn", "no")
            .append_pair("p", &page.max(1).to_string());
        Ok(url.into())
    }

    fn browse(&self, page: u32, popular: bool, filters: &Value) -> Result<Paged<CatalogItem>> {
        if popular && page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let category = Self::category(filters);
        let (document, _) = self.document(&Self::listing_url(category, page, popular))?;
        let result = Self::parse_catalog(&document, popular)?;
        require(
            (!result.entries.is_empty()).then_some(()),
            "52书库 catalog markup is missing",
        )?;
        Ok(result)
    }

    fn parse_catalog(document: &Html, ranked: bool) -> Result<Paged<CatalogItem>> {
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();

        for article in document.select(&selector(".content article.excerpt")?) {
            let Some(link) = article
                .select(&selector("header h2 a[href]")?)
                .find(|link| link_item_url(*link).is_some())
            else {
                continue;
            };
            let description = article
                .select(&selector(".note")?)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            if let Some(item) = catalog_item(link, description, &mut seen)? {
                entries.push(item);
            }
        }

        if ranked || entries.is_empty() {
            for link in document.select(&selector(".content h3 a[href], .content ul li a[href]")?) {
                if let Some(item) = catalog_item(link, None, &mut seen)? {
                    entries.push(item);
                }
            }
        }

        let has_next = document
            .select(&selector(".pagination a.next, .pagination2 a")?)
            .any(|link| normalize_space(&html::text(link)).contains("下一页"));
        Ok(Paged::new(entries, has_next && !ranked))
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let raw_title = first_text(document, "h1.article-title")?
            .ok_or_else(|| Error::new("52书库 novel title is missing"))?;
        let metadata = title_metadata(&raw_title)?;
        let mut item = CatalogItem::new(page_url, metadata.title);
        item.url = Some(page_url.into());
        item.authors = metadata.author.into_iter().collect();
        item.status = Some(json!(metadata.status));
        item.language = Some("zh".into());
        item.initialized = true;

        let mut descriptions = Vec::new();
        for paragraph in document.select(&selector("article.article-content > p")?) {
            if attr(paragraph, "class").is_some_and(|value| value.contains("con_pc")) {
                continue;
            }
            let value = normalize_space(&html::text(paragraph));
            if value.is_empty()
                || value == "小说简介："
                || value.starts_with("所属专题：")
                || value.starts_with("Tips：")
            {
                continue;
            }
            descriptions.push(value);
        }
        item.description = (!descriptions.is_empty()).then(|| descriptions.join("\n\n"));

        let mut seen = BTreeSet::new();
        for link in document.select(&selector(
            "article.article-content a[href*='/Tags'], .breadcrumbs a",
        )?) {
            let value = normalize_space(&html::text(link));
            if value.is_empty()
                || value == "52书库"
                || value.contains("小说在线阅读")
                || !seen.insert(value.clone())
            {
                continue;
            }
            item.tags.push(value);
        }
        item.content_rating = Some(content_rating(
            item.tags
                .iter()
                .map(String::as_str)
                .chain(item.description.iter().map(String::as_str)),
        ));
        Ok(item)
    }

    fn parse_chapters(document: &Html) -> Result<Vec<NovelChapter>> {
        let mut chapters = Vec::new();
        let mut seen = BTreeSet::new();
        for link in document.select(&selector("article.article-content li.mulu a[href]")?) {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let url = site_url(&absolute_url(BASE_URL, &href)?)?.to_string();
            let Some(number) = reading_page_number(&url)? else {
                continue;
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            let title = normalize_space(&html::text(link));
            chapters.push(NovelChapter {
                key: url.clone(),
                url: Some(url),
                title: Some(if title.is_empty() {
                    format!("第{number}页")
                } else {
                    title
                }),
                chapter_number: Some(number as f32),
                language: Some("zh".into()),
                ..NovelChapter::default()
            });
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "52书库 novel has no readable pages",
        )?;
        chapters.reverse();
        Ok(chapters)
    }

    fn parse_text(document: &Html, chapter: &NovelChapter, chapter_url: &str) -> Result<NovelText> {
        let mut paragraphs = Vec::new();
        for paragraph in document.select(&selector("article.article-content > p")?) {
            let value = normalize_space(&html::text(paragraph));
            if value.is_empty()
                || value.starts_with("52书库App下载")
                || value.starts_with("Tips：")
                || value.contains("阅读记录") && value.len() < 80
            {
                continue;
            }
            paragraphs.push(format!("<p>{}</p>", escape_html(&value)));
        }
        require(
            (!paragraphs.is_empty()).then_some(()),
            "52书库 page has no readable text",
        )?;
        let rendered = paragraphs.join("\n");
        Ok(NovelText {
            title: chapter.title.clone(),
            html: Some(rendered.clone()),
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

impl NovelSource for Shuku52Source {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, true, &json!({}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, false, &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.browse(page, true, filters),
            "latest" => self.browse(page, false, filters),
            _ => Err(Error::new(format!("unknown 52书库 listing {listing:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return Ok(Paged::new(Vec::new(), false));
        }
        let (document, _) = self.document(&Self::search_url(query, page)?)?;
        Self::parse_catalog(&document, false)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &canonical_item_url(&final_url)?)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = Self::item_url(&item)?;
        let (document, _) = self.document(&url)?;
        Self::parse_chapters(&document)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = site_url(&absolute_url(
            BASE_URL,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        )?)?
        .to_string();
        require(
            reading_page_number(&url)?.map(|_| ()),
            "52书库 chapter URL is not a reading page",
        )?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_text(&document, &chapter, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "category".into(),
            name: "分类".into(),
            options: CATEGORIES
                .iter()
                .map(|category| OptionItem {
                    label: category.label.into(),
                    value: category.value.into(),
                })
                .collect(),
            default_index: 0,
        }])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(url) = site_url(candidate) else {
            return Ok(None);
        };
        if !is_item_path(url.path()) {
            return Ok(None);
        }
        let item_url = canonical_item_url(url.as_str())?;
        let mut item = CatalogItem::new(&item_url, "");
        item.url = Some(item_url);
        item.language = Some("zh".into());
        let chapter = reading_page_number(url.as_str())?.map(|number| NovelChapter {
            key: url.to_string(),
            url: Some(url.to_string()),
            title: Some(format!("第{number}页")),
            chapter_number: Some(number as f32),
            language: Some("zh".into()),
            ..NovelChapter::default()
        });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter: chapter,
            ..UrlResolveResult::default()
        }))
    }
}

#[derive(Debug, PartialEq)]
struct TitleMetadata {
    title: String,
    author: Option<String>,
    status: &'static str,
}

fn catalog_item(
    link: ElementRef<'_>,
    description: Option<String>,
    seen: &mut BTreeSet<String>,
) -> Result<Option<CatalogItem>> {
    let Some(url) = link_item_url(link) else {
        return Ok(None);
    };
    let url = canonical_item_url(&url)?;
    if !seen.insert(url.clone()) {
        return Ok(None);
    }
    let metadata = title_metadata(&html::text(link))?;
    if metadata.title.is_empty() {
        return Ok(None);
    }
    let mut item = CatalogItem::new(&url, metadata.title);
    item.url = Some(url);
    item.description = description;
    item.authors = metadata.author.into_iter().collect();
    item.status = Some(json!(metadata.status));
    item.language = Some("zh".into());
    item.content_rating = Some(content_rating(
        item.description
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(item.title.as_str())),
    ));
    Ok(Some(item))
}

fn link_item_url(link: ElementRef<'_>) -> Option<String> {
    let href = attr(link, "href")?;
    let absolute = absolute_url(BASE_URL, &href).ok()?;
    let url = site_url(&absolute).ok()?;
    is_item_path(url.path()).then(|| url.to_string())
}

fn site_url(candidate: &str) -> Result<Url> {
    let mut url = Url::parse(candidate).map_err(|_| Error::new("Invalid 52书库 URL"))?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("52shuku.net" | "www.52shuku.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(Error::new("Not a 52书库 URL"));
    }
    url.set_host(Some("www.52shuku.net"))
        .map_err(|_| Error::new("Invalid 52书库 host"))?;
    url.set_fragment(None);
    Ok(url)
}

fn is_item_path(path: &str) -> bool {
    let parts = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return false;
    }
    let reserved = [
        "tuijian", "Top", "Tags", "Tags_Nan", "zuozhe", "so", "skin", "e", "d",
    ];
    if reserved.contains(&parts[0]) {
        return false;
    }
    Regex::new(r"^[A-Za-z0-9]+(?:_\d+)?\.html$")
        .expect("static item regex")
        .is_match(parts.last().expect("non-empty path"))
}

fn canonical_item_url(candidate: &str) -> Result<String> {
    let mut url = site_url(&absolute_url(BASE_URL, candidate)?)?;
    require(
        is_item_path(url.path()).then_some(()),
        "52书库 item URL is invalid",
    )?;
    let canonical = Regex::new(r"_\d+\.html$")
        .expect("static page suffix regex")
        .replace(url.path(), ".html")
        .into_owned();
    url.set_path(&canonical);
    url.set_query(None);
    Ok(url.to_string())
}

fn reading_page_number(candidate: &str) -> Result<Option<u32>> {
    let url = site_url(candidate)?;
    if !is_item_path(url.path()) {
        return Ok(None);
    }
    Ok(Regex::new(r"_(\d+)\.html$")
        .expect("static page suffix regex")
        .captures(url.path())
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .and_then(|site_page| site_page.checked_sub(1)))
}

fn title_metadata(value: &str) -> Result<TitleMetadata> {
    let rank = Regex::new(r"^\s*\d+\s*[、.．]\s*(?:\[[^\]]+\]\s*)?")
        .map_err(|error| Error::new(error.to_string()))?;
    let status_re =
        Regex::new(r"【([^】]+)】\s*$").map_err(|error| Error::new(error.to_string()))?;
    let normalized = normalize_space(&html::text(html::fragment(value).root_element()));
    let without_rank = rank.replace(&normalized, "");
    let status_text = status_re
        .captures(&without_rank)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    let without_status = status_re.replace(&without_rank, "");
    let (title, author) = without_status
        .rsplit_once('_')
        .map(|(title, author)| (title.trim(), Some(author.trim())))
        .unwrap_or((without_status.trim(), None));
    Ok(TitleMetadata {
        title: title.to_owned(),
        author: author.filter(|value| !value.is_empty()).map(str::to_owned),
        status: normalize_status(status_text.as_deref().unwrap_or("")),
    })
}

fn normalize_status(value: &str) -> &'static str {
    if value.contains("完结") || value.contains("完本") || value.contains("全本") {
        "completed"
    } else if value.contains("连载") {
        "ongoing"
    } else {
        "unknown"
    }
}

fn content_rating<'a>(values: impl Iterator<Item = &'a str>) -> String {
    if values.map(str::to_ascii_lowercase).any(|value| {
        value.contains("adult")
            || value.contains("smut")
            || value.contains("限制级")
            || value.contains("成人")
            || value.contains("肉文")
            || value.contains("高h")
    }) {
        "adult"
    } else {
        "suggestive"
    }
    .into()
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

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, Shuku52Source::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_metadata_description_and_pagination() {
        let document = html::document(include_str!("../tests/fixtures/catalog.html"));
        let page = Shuku52Source::parse_catalog(&document, false).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert!(page.has_next_page);
        let item = &page.entries[0];
        assert_eq!(item.title, "测试小说");
        assert_eq!(item.authors, vec!["测试作者"]);
        assert_eq!(item.status, Some(json!("completed")));
        assert_eq!(item.description.as_deref(), Some("这是一段简介。"));
    }

    #[test]
    fn parses_details_chapters_and_sanitized_text() {
        let document = html::document(include_str!("../tests/fixtures/novel.html"));
        let item = Shuku52Source::parse_details(
            &document,
            "https://www.52shuku.net/yanqing/19_b/test1.html",
        )
        .unwrap();
        assert_eq!(item.title, "测试小说");
        assert_eq!(item.authors, vec!["测试作者"]);
        assert_eq!(item.tags, vec!["言情小说", "甜宠"]);
        assert_eq!(item.status, Some(json!("completed")));
        assert!(item.initialized);

        let chapters = Shuku52Source::parse_chapters(&document).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(2.0));
        assert!(chapters[0].key.ends_with("test1_3.html"));

        let chapter_document = html::document(include_str!("../tests/fixtures/chapter.html"));
        let text = Shuku52Source::parse_text(
            &chapter_document,
            &chapters[1],
            "https://www.52shuku.net/yanqing/19_b/test1_2.html",
        )
        .unwrap();
        let rendered = text.html.unwrap();
        assert!(rendered.contains("<p>第一段 &amp; 内容。</p>"));
        assert!(rendered.contains("<p>第二段。</p>"));
        assert!(!rendered.contains("App下载"));
    }

    #[test]
    fn resolves_item_and_reading_page_urls() {
        let mut source = Shuku52Source::default();
        let item = source
            .handle_url("https://52shuku.net/yanqing/19_b/test1.html")
            .unwrap()
            .unwrap();
        assert!(item.novel_chapter.is_none());
        assert_eq!(
            item.item.unwrap().url.as_deref(),
            Some("https://www.52shuku.net/yanqing/19_b/test1.html")
        );

        let page = source
            .handle_url("https://www.52shuku.net/yanqing/19_b/test1_2.html")
            .unwrap()
            .unwrap();
        assert_eq!(page.novel_chapter.unwrap().chapter_number, Some(1.0));
        assert_eq!(
            page.item.unwrap().url.as_deref(),
            Some("https://www.52shuku.net/yanqing/19_b/test1.html")
        );
    }

    #[test]
    fn rejects_non_catalog_site_pages() {
        let mut source = Shuku52Source::default();
        assert!(source
            .handle_url("https://www.52shuku.net/tuijian/yanqing_top100.html")
            .unwrap()
            .is_none());
        assert!(source
            .handle_url("https://example.com/yanqing/test.html")
            .unwrap()
            .is_none());
    }
}
