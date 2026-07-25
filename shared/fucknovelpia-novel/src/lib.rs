use std::{collections::HashSet, marker::PhantomData};

use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use url::Url;

pub trait Config: 'static {
    const BASE_URL: &'static str;
    const LANGUAGE: &'static str;
    const RAW_DOWNLOADS: bool;
}

pub struct Source<C: Config> {
    client: Client,
    config: PhantomData<C>,
}

impl<C: Config> Default for Source<C> {
    fn default() -> Self {
        Self {
            client: Client::browser(),
            config: PhantomData,
        }
    }
}

impl<C: Config> Source<C> {
    fn response_text(&self, url: &str) -> Result<(String, String)> {
        let response = self
            .client
            .get(url)
            .header("Referer", C::BASE_URL)
            .cookies_for(C::BASE_URL)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((response.text()?.to_owned(), final_url))
    }

    fn document(&self, url: &str) -> Result<(Html, String)> {
        let (text, final_url) = self.response_text(url)?;
        Ok((html::document(&text), final_url))
    }

    fn catalog_document(&self, url: &str) -> Result<(Html, String)> {
        let (text, final_url) = self.response_text(url)?;
        Ok((html::document(&catalog_fragment(&text)), final_url))
    }

    fn browse(&self, page: u32, query: &str, filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{}/search.php", C::BASE_URL.trim_end_matches('/')))
            .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", query.trim());
            pairs.append_pair("author", "");
            pairs.append_pair("uploader", "");
            pairs.append_pair("translator_group", "");
            pairs.append_pair("country", "");
            pairs.append_pair("year_from", "");
            pairs.append_pair("year_to", "");
            pairs.append_pair("status", filter(filters, "status", ""));
            pairs.append_pair("language", filter(filters, "language", ""));
            pairs.append_pair("read_only", filter(filters, "read_only", "any"));
            pairs.append_pair("sort", filter(filters, "sort", "popular"));
            pairs.append_pair("tag_mode", "AND");
            pairs.append_pair("genre_mode", "AND");
            if filters
                .get("has_images")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                pairs.append_pair("has_images", "1");
            }
            if page > 1 {
                pairs.append_pair("page", &page.to_string());
            }
        }
        let (document, _) = self.catalog_document(url.as_str())?;
        Self::parse_cards(&document, page)
    }

    fn latest_updates(&self) -> Result<Paged<CatalogItem>> {
        let url = format!("{}/updates.php", C::BASE_URL.trim_end_matches('/'));
        let (text, _) = self.response_text(&url)?;
        let document = html::document(&updates_fragment(&text, 24));
        let mut page = Self::parse_cards_from(&document, ".updates-list a[href*='/novel/']")?;
        page.has_next_page = false;
        Ok(page)
    }

    fn parse_cards(document: &Html, page: u32) -> Result<Paged<CatalogItem>> {
        let mut result = Self::parse_cards_from(document, ".card-book a[href*='/novel/']")?;
        if page > 1 {
            let active = selector("div.pagination a.active")?;
            let active_page = document
                .select(&active)
                .next()
                .map(html::text)
                .and_then(|value| value.trim().parse::<u32>().ok());
            if active_page != Some(page) {
                result.entries.clear();
                result.has_next_page = false;
                return Ok(result);
            }
        }
        let next = selector("div.pagination a")?;
        result.has_next_page = document.select(&next).any(|anchor| {
            html::text(anchor).trim().parse::<u32>().ok() == Some(page.saturating_add(1))
        });
        Ok(result)
    }

    fn parse_cards_from(document: &Html, selector_value: &str) -> Result<Paged<CatalogItem>> {
        let cards = selector(selector_value)?;
        let image = selector("img")?;
        let title = selector(".title")?;
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for anchor in document.select(&cards) {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let page_url = absolute_url(C::BASE_URL, &href)?;
            if !seen.insert(page_url.clone()) {
                continue;
            }
            let image_node = anchor.select(&image).next();
            let name = image_node
                .and_then(|node| attr(node, "alt"))
                .or_else(|| anchor.select(&title).next().map(html::text))
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Untitled".to_owned());
            let mut item = CatalogItem::new(page_url.clone(), name);
            item.url = Some(page_url.clone());
            item.cover = image_node
                .and_then(|node| attr(node, "src").or_else(|| attr(node, "data-src")))
                .map(|value| absolute_url(C::BASE_URL, &value))
                .transpose()?
                .map(|value| image_request(&value, &page_url));
            item.language = Some(C::LANGUAGE.to_owned());
            item.content_rating = Some("adult".to_owned());
            entries.push(item);
        }
        Ok(Paged::new(entries, false))
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        absolute_url(C::BASE_URL, item.url.as_deref().unwrap_or(&item.key))
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let json_ld = selector("script[type='application/ld+json']")?;
        let metadata = document
            .select(&json_ld)
            .find_map(|element| serde_json::from_str::<Value>(&element.inner_html()).ok());

        let title = metadata
            .as_ref()
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| first_text(document, "h1.hero-title, h1").ok().flatten());
        let title = require(title, "FuckNovelpia novel has no title")?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.to_owned());
        item.description = metadata
            .as_ref()
            .and_then(|value| value.get("description"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| first_text(document, ".hero-summary").ok().flatten())
            .filter(|value| !value.starts_with("No description yet"));
        item.authors = metadata
            .as_ref()
            .and_then(|value| value.get("author"))
            .map(json_authors)
            .filter(|authors| !authors.is_empty())
            .unwrap_or_else(|| {
                info_value(document, "Author")
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect()
            });
        item.tags = metadata
            .as_ref()
            .and_then(|value| value.get("genre"))
            .map(json_strings)
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| texts(document, ".genre-pill").unwrap_or_default());
        item.cover = metadata
            .as_ref()
            .and_then(|value| value.get("image"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                first_attr(document, ".hero-media img, .cover img", "src")
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                first_attr(document, ".hero-media .cover, .cover", "style")
                    .ok()
                    .flatten()
                    .and_then(|style| css_background_url(&style))
            })
            .map(|value| absolute_url(C::BASE_URL, &value))
            .transpose()?
            .map(|value| image_request(&value, page_url));
        item.status =
            first_text(document, ".status-badge")?.map(|value| json!(normalize_status(&value)));
        item.language = Some(C::LANGUAGE.to_owned());
        item.content_rating = Some("adult".to_owned());
        item.initialized = true;
        Ok(item)
    }

    fn parse_chapters(document: &Html, page_url: &str) -> Result<Vec<NovelChapter>> {
        if C::RAW_DOWNLOADS {
            let download = first_attr(document, "a.novel-download-link", "href")?
                .map(|href| absolute_url(C::BASE_URL, &href))
                .transpose()?;
            let Some(download) = download else {
                return Ok(Vec::new());
            };
            return Ok(vec![NovelChapter {
                key: download.clone(),
                title: Some("Download Korean RAW ZIP".to_owned()),
                url: Some(download.clone()),
                language: Some(C::LANGUAGE.to_owned()),
                source_order: Some(0),
                section: Some("RAW download".to_owned()),
                summary: Some(
                    "The source publishes this work as a password-protected ZIP download."
                        .to_owned(),
                ),
                extra: [("downloadUrl".to_owned(), json!(download))]
                    .into_iter()
                    .collect(),
                ..NovelChapter::default()
            }]);
        }

        let rows = selector("#chapter-list li")?;
        let anchor_selector = selector("a[href]")?;
        let title_selector = selector(".chapter-item-main")?;
        let image_flag = selector(".chapter-item-flag")?;
        let mut chapters = Vec::new();
        for (index, row) in document.select(&rows).enumerate() {
            let Some(anchor) = row.select(&anchor_selector).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let url = absolute_url(page_url, &href)?;
            let title = row
                .select(&title_selector)
                .next()
                .map(html::text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| Some(format!("Chapter {}", index + 1)));
            let chapter_number = attr(row, "data-ch").and_then(|value| value.parse().ok());
            let mut extra = manatan_sdk::model::Extra::default();
            if row.select(&image_flag).next().is_some() {
                extra.insert("hasImages".to_owned(), json!(true));
            }
            chapters.push(NovelChapter {
                key: url.clone(),
                title,
                chapter_number,
                url: Some(url),
                language: Some(C::LANGUAGE.to_owned()),
                source_order: Some(index as i32),
                extra,
                ..NovelChapter::default()
            });
        }
        Ok(chapters)
    }

    fn parse_text(document: &Html, page_url: &str, chapter: &NovelChapter) -> Result<NovelText> {
        if C::RAW_DOWNLOADS {
            let download = chapter
                .extra
                .get("downloadUrl")
                .and_then(Value::as_str)
                .or(chapter.url.as_deref())
                .unwrap_or(&chapter.key)
                .to_owned();
            return Ok(NovelText {
                html: Some(
                    "<p>This source publishes the Korean original as a password-protected ZIP. \
                     Open the source download page to continue.</p>"
                        .to_owned(),
                ),
                text: Some(
                    "This source publishes the Korean original as a password-protected ZIP."
                        .to_owned(),
                ),
                title: chapter.title.clone(),
                base_url: Some(page_url.to_owned()),
                blocks: vec![NovelContentBlock::PageUrl { url: download }],
                ..NovelText::default()
            });
        }

        let reader = selector(".reader")?;
        let reader = require(
            document.select(&reader).next(),
            "FuckNovelpia chapter has no reader content",
        )?;
        let content =
            selector("p, h1, h2, h3, h4, h5, h6, blockquote, li, pre, div.chapter-image")?;
        let images = selector("img[src], img[data-src]")?;
        let mut blocks = Vec::new();
        let mut paragraphs = Vec::new();
        for node in reader.select(&content) {
            if has_ancestor_class(node, "reader-nav") {
                continue;
            }
            let value = normalize_space(&html::text(node));
            if !value.is_empty() {
                paragraphs.push(value.clone());
                blocks.push(NovelContentBlock::Text {
                    text: value,
                    html: false,
                });
            }
        }
        for image in reader.select(&images) {
            if has_ancestor_class(image, "reader-nav") {
                continue;
            }
            let Some(src) = attr(image, "src").or_else(|| attr(image, "data-src")) else {
                continue;
            };
            let url = absolute_url(page_url, &src)?;
            blocks.push(NovelContentBlock::Image {
                image: image_request(&url, page_url),
                alt: attr(image, "alt"),
            });
        }
        require(
            (!blocks.is_empty()).then_some(()),
            "FuckNovelpia chapter reader is empty",
        )?;
        let html = (!paragraphs.is_empty()).then(|| {
            paragraphs
                .iter()
                .map(|value| format!("<p>{}</p>", escape_html(value)))
                .collect::<Vec<_>>()
                .join("\n")
        });
        Ok(NovelText {
            html,
            text: (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n")),
            title: chapter.title.clone(),
            base_url: Some(page_url.to_owned()),
            image_context: Some(ImageRequestContext {
                headers: [
                    ("Referer".to_owned(), page_url.to_owned()),
                    (
                        "User-Agent".to_owned(),
                        manatan_sdk::client::BROWSER_USER_AGENT.to_owned(),
                    ),
                ]
                .into_iter()
                .collect(),
                cookie_url: Some(C::BASE_URL.to_owned()),
            }),
            blocks,
            ..NovelText::default()
        })
    }
}

impl<C: Config> NovelSource for Source<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse(page, "", &json!({"sort": "popular"}))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::default());
        }
        self.latest_updates()
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(page, query, filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let page_url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&page_url)?;
        Self::parse_details(&document, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let page_url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&page_url)?;
        Self::parse_chapters(&document, &final_url)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let page_url = absolute_url(C::BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        if C::RAW_DOWNLOADS {
            return Self::parse_text(&html::document(""), &page_url, &chapter);
        }
        let (document, final_url) = self.document(&page_url)?;
        Self::parse_text(&document, &final_url, &chapter)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select(
                "sort",
                "Sort",
                &[
                    ("Newest", "newest"),
                    ("Popular", "popular"),
                    ("Oldest", "oldest"),
                    ("Title A-Z", "title"),
                    ("Year (descending)", "year_desc"),
                    ("Year (ascending)", "year_asc"),
                ],
                0,
            ),
            select(
                "status",
                "Status",
                &[
                    ("Any", ""),
                    ("Ongoing", "ongoing"),
                    ("Completed", "completed"),
                    ("Hiatus", "hiatus"),
                    ("Dropped", "dropped"),
                ],
                0,
            ),
            select(
                "language",
                "Language",
                &[
                    ("Any", ""),
                    ("English", "en"),
                    ("Korean", "ko"),
                    ("Japanese", "ja"),
                    ("Chinese", "zh"),
                    ("Spanish", "es"),
                ],
                0,
            ),
            FilterDefinition::CheckBox {
                id: "has_images".to_owned(),
                name: "Image chapters".to_owned(),
                default: false,
            },
            select(
                "read_only",
                "Read mode",
                &[("Any", "any"), ("Read only", "yes"), ("Downloadable", "no")],
                0,
            ),
        ])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(Self::item_url(item)?))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &NovelChapter,
    ) -> Result<Option<String>> {
        Ok(Some(absolute_url(
            C::BASE_URL,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        )?))
    }

    fn handle_url(&mut self, url: &str) -> Result<Option<UrlResolveResult>> {
        let parsed = Url::parse(url).map_err(|error| Error::new(error.to_string()))?;
        let base = Url::parse(C::BASE_URL).map_err(|error| Error::new(error.to_string()))?;
        if parsed.host_str() != base.host_str() {
            return Ok(None);
        }
        if parsed.path().starts_with("/novel/") {
            let mut item = CatalogItem::new(parsed.to_string(), "");
            item.url = Some(parsed.to_string());
            item.language = Some(C::LANGUAGE.to_owned());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        if parsed.path().contains("chapter.php") || parsed.path().contains("download.php") {
            let chapter = NovelChapter {
                key: parsed.to_string(),
                url: Some(parsed.to_string()),
                language: Some(C::LANGUAGE.to_owned()),
                ..NovelChapter::default()
            };
            return Ok(Some(UrlResolveResult {
                novel_chapter: Some(chapter),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str {
    filters.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn strip_script_blocks(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(start) = remaining.find("<script") {
        output.push_str(&remaining[..start]);
        let Some(end) = remaining[start..].find("</script>") else {
            return output;
        };
        remaining = &remaining[start + end + "</script>".len()..];
    }
    output.push_str(remaining);
    output
}

fn catalog_fragment(input: &str) -> String {
    let stripped = strip_script_blocks(input);
    let Some(grid_start) = stripped.find("<div class=\"grid\">") else {
        return stripped;
    };
    let Some(pagination_offset) = stripped[grid_start..].find("<div class=\"pagination\">") else {
        return stripped[grid_start..].to_owned();
    };
    let pagination_start = grid_start + pagination_offset;
    let Some(pagination_end_offset) = stripped[pagination_start..].find("</div>") else {
        return stripped[grid_start..].to_owned();
    };
    stripped[grid_start..pagination_start + pagination_end_offset + "</div>".len()].to_owned()
}

fn updates_fragment(input: &str, limit: usize) -> String {
    let stripped = strip_script_blocks(input);
    let Some(start) = stripped.find("<div class=\"updates-list\">") else {
        return stripped;
    };
    let updates = &stripped[start..];
    let mut cursor = 0;
    for _ in 0..limit {
        let Some(end) = updates[cursor..].find("</a>") else {
            return updates.to_owned();
        };
        cursor += end + "</a>".len();
    }
    format!("{}</div>", &updates[..cursor])
}

fn select(id: &str, name: &str, options: &[(&str, &str)], default_index: u32) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_owned(),
        name: name.to_owned(),
        options: options
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).to_owned(),
                value: (*value).to_owned(),
            })
            .collect(),
        default_index,
    }
}

fn image_request(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url)
        .cookies_for(url)
        .header("Referer", referer)
        .header("User-Agent", manatan_sdk::client::BROWSER_USER_AGENT)
}

fn first_text(document: &Html, selector_value: &str) -> Result<Option<String>> {
    let value = selector(selector_value)?;
    Ok(document
        .select(&value)
        .next()
        .map(html::text)
        .map(|text| normalize_space(&text))
        .filter(|text| !text.is_empty()))
}

fn texts(document: &Html, selector_value: &str) -> Result<Vec<String>> {
    let value = selector(selector_value)?;
    Ok(document
        .select(&value)
        .map(html::text)
        .map(|text| normalize_space(&text))
        .filter(|text| !text.is_empty())
        .collect())
}

fn first_attr(document: &Html, selector_value: &str, name: &str) -> Result<Option<String>> {
    let value = selector(selector_value)?;
    Ok(document
        .select(&value)
        .find_map(|element| attr(element, name)))
}

fn json_strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => vec![value.to_owned()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn json_authors(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect(),
        Value::Array(values) => values.iter().flat_map(json_authors).collect(),
        _ => Vec::new(),
    }
}

fn info_value(document: &Html, label: &str) -> Option<String> {
    let selector = selector(".info-list li").ok()?;
    document.select(&selector).find_map(|element| {
        let value = normalize_space(&html::text(element));
        let prefix = format!("{label}:");
        value
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "—")
            .map(str::to_owned)
    })
}

fn css_background_url(style: &str) -> Option<String> {
    Regex::new(r#"(?i)background-image\s*:\s*url\(['"]?([^'")]+)"#)
        .ok()?
        .captures(style)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
}

fn normalize_status(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" => "ongoing",
        "completed" => "completed",
        "hiatus" => "hiatus",
        "dropped" => "cancelled",
        _ => "unknown",
    }
}

fn has_ancestor_class(element: manatan_sdk::html::ElementRef<'_>, class: &str) -> bool {
    element.ancestors().any(|node| {
        manatan_sdk::html::ElementRef::wrap(node)
            .is_some_and(|element| element.value().classes().any(|value| value == class))
    })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct English;
    impl Config for English {
        const BASE_URL: &'static str = "https://fucknovelpia.com";
        const LANGUAGE: &'static str = "en";
        const RAW_DOWNLOADS: bool = false;
    }

    struct Korean;
    impl Config for Korean {
        const BASE_URL: &'static str = "https://raw-fucknovelpia.com";
        const LANGUAGE: &'static str = "ko";
        const RAW_DOWNLOADS: bool = true;
    }

    #[test]
    fn parses_catalog_cards_without_duplicates() {
        let document = html::document(
            r#"<article class="card-book"><a href="/novel/test">
            <img src="/cover.jpg" alt="Test Novel"><span class="title">Ignored</span>
            </a></article><article class="card-book"><a href="/novel/test">duplicate</a></article>"#,
        );
        let page = Source::<English>::parse_cards(&document, 1).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].title, "Test Novel");
        assert_eq!(
            page.entries[0].cover.as_ref().unwrap().url,
            "https://fucknovelpia.com/cover.jpg"
        );
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .unwrap()
                .cookie_url
                .as_deref(),
            Some("https://fucknovelpia.com/cover.jpg")
        );
    }

    #[test]
    fn strips_large_catalog_scripts_before_html_parsing() {
        let html = format!(
            "<html><script>const titles = [{}];</script><body><a href=\"/novel/1\">One</a></body></html>",
            "\"large title\",".repeat(50_000)
        );
        let stripped = strip_script_blocks(&html);
        assert!(!stripped.contains("large title"));
        assert!(stripped.contains("<a href=\"/novel/1\">One</a>"));
    }

    #[test]
    fn parses_only_the_catalog_grid_and_pagination() {
        let html = r#"<html><body>
            <form><datalist><option value="thousands of unused filters"></option></datalist></form>
            <div class="grid"><article class="card-book"><a href="/novel/1">One</a></article></div>
            <div class="pagination"><a class="active">1</a><a>2</a></div>
            <footer>unused footer</footer>
        </body></html>"#;
        let fragment = catalog_fragment(html);
        assert!(fragment.starts_with("<div class=\"grid\">"));
        assert!(fragment.ends_with("</div>"));
        assert!(fragment.contains("card-book"));
        assert!(fragment.contains("pagination"));
        assert!(!fragment.contains("datalist"));
        assert!(!fragment.contains("unused footer"));
    }

    #[test]
    fn limits_latest_updates_before_html_parsing() {
        let html = format!(
            "<header>unused</header><div class=\"updates-list\">{}</div><footer>unused</footer>",
            (1..=30)
                .map(|index| format!("<a href=\"/novel/{index}\">Novel {index}</a>"))
                .collect::<String>()
        );
        let fragment = updates_fragment(&html, 24);
        assert!(fragment.starts_with("<div class=\"updates-list\">"));
        assert!(fragment.contains("/novel/24"));
        assert!(!fragment.contains("/novel/25"));
        assert!(!fragment.contains("unused"));
        let document = html::document(&fragment);
        let page =
            Source::<English>::parse_cards_from(&document, ".updates-list a[href*='/novel/']")
                .unwrap();
        assert_eq!(page.entries.len(), 24);
    }

    #[test]
    fn parses_english_chapters_and_reader_content() {
        let document = html::document(
            r#"<ul id="chapter-list"><li data-ch="2"><a href="/chapter.php?x=2">
            <span class="chapter-item-main">Chapter 2</span></a></li></ul>
            <div class="reader"><nav class="reader-nav"><p>Skip</p></nav>
            <p>Hello <b>world</b>.</p><img src="/image.jpg" alt="art"></div>"#,
        );
        let chapters = Source::<English>::parse_chapters(&document, English::BASE_URL).unwrap();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].chapter_number, Some(2.0));
        let text = Source::<English>::parse_text(
            &document,
            "https://fucknovelpia.com/chapter.php?x=2",
            &chapters[0],
        )
        .unwrap();
        assert_eq!(text.text.as_deref(), Some("Hello world ."));
        assert_eq!(text.blocks.len(), 2);
    }

    #[test]
    fn exposes_korean_raw_download_as_a_host_page() {
        let document = html::document(
            r#"<a class="novel-download-link" href="/download.php?slug=1">Download</a>"#,
        );
        let chapters = Source::<Korean>::parse_chapters(&document, Korean::BASE_URL).unwrap();
        assert_eq!(chapters.len(), 1);
        let text = Source::<Korean>::parse_text(&document, Korean::BASE_URL, &chapters[0]).unwrap();
        assert!(matches!(
            text.blocks.as_slice(),
            [NovelContentBlock::PageUrl { .. }]
        ));
    }
}
