// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' Madara novel implementation.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use manatan_common::{absolute_url, extract_number, normalize_space, require};
use manatan_sdk::{
    client::Client,
    html::{self, ElementRef, Html, Selector},
    model::{
        CatalogItem, FilterDefinition, NovelChapter, NovelContentBlock, NovelText, Paged,
        SortOption, SortSelection, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/104.0.0.0 Safari/537.36";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MadaraPaths {
    pub novels: &'static str,
    pub novel: &'static str,
    pub chapter: &'static str,
}

impl Default for MadaraPaths {
    fn default() -> Self {
        Self {
            novels: "novel",
            novel: "novel",
            chapter: "novel",
        }
    }
}

/// Site-specific configuration for an IReader-style Madara novel source.
///
/// Defaults model the upstream family. Leaf sources should override only the
/// constants or selectors their site actually changes.
pub trait MadaraNovelConfig: Default + 'static {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const LANG: &'static str;

    fn paths(&self) -> MadaraPaths {
        MadaraPaths::default()
    }

    fn user_agent(&self) -> &'static str {
        DEFAULT_USER_AGENT
    }

    fn list_selector(&self) -> &'static str {
        ".page-item-detail"
    }

    fn search_selector(&self) -> &'static str {
        "div.c-tabs-item__content"
    }

    fn chapter_selector(&self) -> &'static str {
        "li.wp-manga-chapter"
    }

    fn content_selector(&self) -> &'static str {
        ".text-left p, .text-right p"
    }

    fn chapter_url_markers(&self) -> &'static [&'static str] {
        &["/chapter-", "/chapter/"]
    }
}

pub struct MadaraNovelSource<C> {
    client: Client,
    config: C,
}

impl<C: MadaraNovelConfig> Default for MadaraNovelSource<C> {
    fn default() -> Self {
        Self::new(C::default())
    }
}

impl<C: MadaraNovelConfig> MadaraNovelSource<C> {
    pub fn new(config: C) -> Self {
        let client = Client::new()
            .header("User-Agent", config.user_agent())
            .header("Cache-Control", "max-age=0")
            .header("Referer", C::BASE_URL);
        Self { client, config }
    }

    pub fn listing_url(&self, page: u32, sort_index: u32) -> String {
        let order = match sort_index {
            1 => "alphabet",
            2 => "raing",
            3 => "trending",
            4 => "views",
            _ => "latest",
        };
        format!(
            "{}/{}/page/{}/?m_orderby={order}",
            C::BASE_URL.trim_end_matches('/'),
            self.config.paths().novels.trim_matches('/'),
            page.max(1)
        )
    }

    pub fn search_url(&self, query: &str, page: u32) -> Result<String> {
        let mut url = Url::parse(C::BASE_URL).map_err(|error| Error::new(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("s", query)
                .append_pair("post_type", "wp-manga")
                .append_pair("op", "")
                .append_pair("author", "")
                .append_pair("artist", "")
                .append_pair("release", "")
                .append_pair("adult", "");
            if page > 1 {
                pairs.append_pair("paged", &page.to_string());
            }
        }
        Ok(url.to_string())
    }

    pub fn chapter_endpoint_urls(&self) -> [String; 2] {
        let base = C::BASE_URL.trim_end_matches('/');
        [
            format!("{base}/wp-admin/admin-ajax.php"),
            format!("{base}/ajax/chapters/"),
        ]
    }

    pub fn parse_chapter_candidates<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a str>,
        now_millis: i64,
    ) -> Result<Vec<NovelChapter>> {
        for candidate in candidates {
            let chapters = self.parse_chapters_html(candidate, now_millis)?;
            if !chapters.is_empty() {
                return Ok(chapters);
            }
        }
        Ok(Vec::new())
    }

    pub fn parse_list_html(&self, source: &str, search: bool) -> Result<Paged<CatalogItem>> {
        let document = html::document(source);
        let item_selector = selector(if search {
            self.config.search_selector()
        } else {
            self.config.list_selector()
        })?;
        let title_link = selector(".post-title a, div.post-title h3.h4 a")?;
        let image = selector("img")?;
        let next = selector("div.nav-previous>a")?;

        let mut entries = Vec::new();
        for element in document.select(&item_selector) {
            let Some(anchor) = element.select(&title_link).next() else {
                continue;
            };
            let title = normalize_space(&html::text(anchor));
            let Some(href) = attribute(anchor, "href") else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(C::BASE_URL, &href)?;
            let cover = element.select(&image).next().and_then(|image| {
                attribute(image, "data-src")
                    .or_else(|| attribute(image, "data-lazy-src"))
                    .or_else(|| attribute(image, "src"))
                    .and_then(|candidate| absolute_url(C::BASE_URL, &candidate).ok())
            });
            let mut item = CatalogItem::new(url.clone(), title);
            item.url = Some(url);
            item.cover = cover.map(Into::into);
            item.language = Some(C::LANG.to_owned());
            item.extra.insert(
                "coverHeaders".to_owned(),
                json!({"User-Agent": self.config.user_agent(), "Referer": C::BASE_URL}),
            );
            entries.push(item);
        }

        Ok(Paged::new(entries, document.select(&next).next().is_some()))
    }

    pub fn parse_details_html(&self, source: &str, page_url: &str) -> Result<CatalogItem> {
        let document = html::document(source);
        let title = required_text(
            &document,
            "div.post-title>h1",
            "Madara detail page has no title",
        )?;
        let image_selector = selector("div.summary_image a img")?;
        let cover = document.select(&image_selector).next().and_then(|image| {
            let src = attribute(image, "src");
            let candidate = match src {
                Some(value) if !value.to_ascii_lowercase().contains("data:image/svg+xml") => {
                    Some(value)
                }
                _ => attribute(image, "data-src").or_else(|| attribute(image, "data-lazy-src")),
            }?;
            absolute_url(C::BASE_URL, &candidate).ok()
        });
        let author_selector = selector("div.author-content>a")?;
        let authors = document
            .select(&author_selector)
            .next()
            .map(|element| {
                attribute(element, "title")
                    .unwrap_or_else(|| html::text(element))
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let description = description(&document)?;
        let genres = all_text(&document, "div.genres-content a")?;
        let rating = optional_text(&document, "div.post-rating span.score")?
            .and_then(|value| value.parse::<f32>().ok());
        let status_text =
            optional_text(&document, "div.post-status div.summary-content")?.unwrap_or_default();
        let status = match status_text.to_ascii_lowercase().as_str() {
            value if value.contains("ongoing") || value.contains("مستمرة") => {
                Some(json!("ongoing"))
            }
            value if value.contains("completed") => Some(json!("completed")),
            _ => Some(json!("unknown")),
        };
        let url = absolute_url(C::BASE_URL, page_url)?;

        let mut item = CatalogItem::new(url.clone(), title);
        item.url = Some(url);
        item.cover = cover.map(Into::into);
        item.description = description;
        item.authors = authors;
        item.tags = genres;
        item.rating = rating;
        item.status = status;
        item.initialized = true;
        item.language = Some(C::LANG.to_owned());
        Ok(item)
    }

    pub fn parse_chapters_html(&self, source: &str, now_millis: i64) -> Result<Vec<NovelChapter>> {
        let document = html::document(source);
        let chapters = selector(self.config.chapter_selector())?;
        let anchor = selector("a")?;
        let date = selector("i")?;
        let mut entries = Vec::new();
        for element in document.select(&chapters) {
            let Some(link) = element.select(&anchor).next() else {
                continue;
            };
            let Some(href) = attribute(link, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(link));
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(C::BASE_URL, &href)?;
            let date_text = element
                .select(&date)
                .next()
                .map(html::text)
                .unwrap_or_default();
            entries.push(NovelChapter {
                key: url.clone(),
                title: Some(title.clone()),
                chapter_number: extract_number(&title),
                date_uploaded: parse_date(&date_text, now_millis),
                url: Some(url),
                language: Some(C::LANG.to_owned()),
                ..NovelChapter::default()
            });
        }
        entries.reverse();
        for (index, chapter) in entries.iter_mut().enumerate() {
            chapter.source_order = Some(index as i32);
        }
        Ok(entries)
    }

    pub fn parse_text_html(&self, source: &str, page_url: &str) -> Result<NovelText> {
        let document = html::document(source);
        let title = optional_text(&document, ".text-center")?
            .filter(|value| !value.is_empty())
            .or(optional_text(&document, "#chapter-heading")?);
        let paragraphs = all_text(&document, self.config.content_selector())?
            .into_iter()
            .map(|value| normalize_space(&value.replace("Read latest Chapters at", "")))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if title.as_deref().unwrap_or_default().is_empty() && paragraphs.is_empty() {
            return Err(Error::new("Madara chapter page contains no readable text"));
        }

        let mut blocks = Vec::new();
        if let Some(title) = title.as_ref().filter(|value| !value.is_empty()) {
            blocks.push(NovelContentBlock::Text {
                text: title.clone(),
                html: false,
            });
        }
        blocks.extend(
            paragraphs
                .iter()
                .cloned()
                .map(|text| NovelContentBlock::Text { text, html: false }),
        );
        let mut text_parts = Vec::new();
        if let Some(title) = title.as_ref().filter(|value| !value.is_empty()) {
            text_parts.push(title.clone());
        }
        text_parts.extend(paragraphs);

        Ok(NovelText {
            text: Some(text_parts.join("\n\n")),
            title,
            base_url: Some(absolute_url(C::BASE_URL, page_url)?),
            blocks,
            ..NovelText::default()
        })
    }

    fn get_html(&self, url: &str) -> Result<(String, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((response.text()?.to_owned(), response.final_url().to_owned()))
    }

    fn post_chapters(
        &self,
        url: &str,
        book_id: &str,
        now_millis: i64,
    ) -> Result<Vec<NovelChapter>> {
        let response = self
            .client
            .post(url)
            .form(&[("action", "manga_get_chapters"), ("manga", book_id)])
            .send()?
            .error_for_status()?;
        self.parse_chapters_html(response.text()?, now_millis)
    }

    fn fetch_chapters(&self, item: &CatalogItem) -> Result<Vec<NovelChapter>> {
        let detail_url = item.url.as_deref().unwrap_or(&item.key);
        let (detail_html, _) = self.get_html(detail_url)?;
        let detail_document = html::document(&detail_html);
        let id_selector = selector(".rating-post-id")?;
        let book_id = detail_document
            .select(&id_selector)
            .next()
            .and_then(|element| attribute(element, "value"))
            .unwrap_or_default();
        let now_millis = manatan_sdk::host::now_millis();

        if !book_id.is_empty() {
            let [admin_url, ajax_url] = self.chapter_endpoint_urls();
            if let Ok(chapters) = self.post_chapters(&admin_url, &book_id, now_millis) {
                if !chapters.is_empty() {
                    return Ok(chapters);
                }
            }
            if let Ok(chapters) = self.post_chapters(&ajax_url, &book_id, now_millis) {
                if !chapters.is_empty() {
                    return Ok(chapters);
                }
            }
        }
        self.parse_chapters_html(&detail_html, now_millis)
    }

    fn sort_index(filters: &Value) -> u32 {
        filters
            .get("sort")
            .and_then(|value| {
                value
                    .get("index")
                    .and_then(Value::as_u64)
                    .or_else(|| value.as_u64())
            })
            .or_else(|| filters.get("sortIndex").and_then(Value::as_u64))
            .unwrap_or_default() as u32
    }

    fn title_filter(filters: &Value) -> Option<&str> {
        filters
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn command_html<'a>(request: &'a Value, operation: &str) -> Result<&'a str> {
        request
            .get("html")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                Error::new(format!(
                    "{operation} requires non-empty string field \"html\""
                ))
            })
    }
}

impl<C: MadaraNovelConfig> NovelSource for MadaraNovelSource<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let (source, _) = self.get_html(&self.listing_url(page, 0))?;
        self.parse_list_html(&source, false)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.popular(page)
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if !matches!(listing, "popular" | "latest") {
            return Err(Error::new(format!("unknown novel listing {listing:?}")));
        }
        if let Some(query) = Self::title_filter(filters) {
            return self.search(query, page, filters);
        }
        let url = self.listing_url(page, Self::sort_index(filters));
        let (source, _) = self.get_html(&url)?;
        self.parse_list_html(&source, false)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = if query.trim().is_empty() {
            Self::title_filter(filters).unwrap_or_default()
        } else {
            query.trim()
        };
        let (source, _) = self.get_html(&self.search_url(query, page)?)?;
        self.parse_list_html(&source, true)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        let (source, final_url) = self.get_html(url)?;
        self.parse_details_html(&source, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        self.fetch_chapters(&item)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (source, final_url) = self.get_html(url)?;
        self.parse_text_html(&source, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            FilterDefinition::Text {
                id: "title".to_owned(),
                name: "Title".to_owned(),
                default: String::new(),
            },
            FilterDefinition::Sort {
                id: "sort".to_owned(),
                name: "Sort By".to_owned(),
                options: vec![
                    SortOption {
                        label: "Latest".to_owned(),
                        value: "latest".to_owned(),
                    },
                    SortOption {
                        label: "A-Z".to_owned(),
                        value: "alphabet".to_owned(),
                    },
                    SortOption {
                        label: "Rating".to_owned(),
                        value: "raing".to_owned(),
                    },
                    SortOption {
                        label: "Trending".to_owned(),
                        value: "trending".to_owned(),
                    },
                    SortOption {
                        label: "Most Views".to_owned(),
                        value: "views".to_owned(),
                    },
                    SortOption {
                        label: "New".to_owned(),
                        value: "latest".to_owned(),
                    },
                ],
                default: Some(SortSelection {
                    index: 0,
                    ascending: false,
                }),
            },
        ])
    }

    fn dispatch(&mut self, operation: &str, request: &Value) -> Result<Option<Value>> {
        match operation {
            "commands.describe" => Ok(Some(json!({
                "commands": [
                    {"id": "commands.detail.fetch", "category": "detail", "inputs": ["url", "html"]},
                    {"id": "commands.chapter.fetch", "category": "chapter", "inputs": ["url", "html"]},
                    {"id": "commands.content.fetch", "category": "content", "inputs": ["url", "html"]}
                ]
            }))),
            "commands.detail.fetch" => {
                let source = Self::command_html(request, operation)?;
                let url = request
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(C::BASE_URL);
                Ok(Some(serde_json::to_value(
                    self.parse_details_html(source, url)?,
                )?))
            }
            "commands.chapter.fetch" => {
                let source = Self::command_html(request, operation)?;
                let now = request
                    .get("nowMillis")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(manatan_sdk::host::now_millis);
                Ok(Some(serde_json::to_value(
                    self.parse_chapters_html(source, now)?,
                )?))
            }
            "commands.content.fetch" => {
                let source = Self::command_html(request, operation)?;
                let url = request
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(C::BASE_URL);
                Ok(Some(serde_json::to_value(
                    self.parse_text_html(source, url)?,
                )?))
            }
            _ => Ok(None),
        }
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        Ok(Some(absolute_url(C::BASE_URL, candidate)?))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &NovelChapter,
    ) -> Result<Option<String>> {
        let candidate = chapter.url.as_deref().unwrap_or(&chapter.key);
        Ok(Some(absolute_url(C::BASE_URL, candidate)?))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let base = Url::parse(C::BASE_URL).map_err(|error| Error::new(error.to_string()))?;
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if base.scheme() != url.scheme()
            || base.host_str() != url.host_str()
            || base.port_or_known_default() != url.port_or_known_default()
        {
            return Ok(None);
        }
        let canonical = url.to_string();
        if self
            .config
            .chapter_url_markers()
            .iter()
            .any(|marker| url.path().contains(marker))
        {
            return Ok(Some(UrlResolveResult {
                chapter_key: Some(canonical.clone()),
                novel_chapter: Some(NovelChapter {
                    key: canonical.clone(),
                    url: Some(canonical),
                    language: Some(C::LANG.to_owned()),
                    ..NovelChapter::default()
                }),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            item: Some(CatalogItem {
                key: canonical.clone(),
                url: Some(canonical),
                language: Some(C::LANG.to_owned()),
                ..CatalogItem::default()
            }),
            ..UrlResolveResult::default()
        }))
    }
}

fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}

fn attribute(element: ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_text(document: &Html, selector_value: &str) -> Result<Option<String>> {
    let selector = selector(selector_value)?;
    Ok(document
        .select(&selector)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn required_text(document: &Html, selector_value: &str, message: &str) -> Result<String> {
    require(optional_text(document, selector_value)?, message)
}

fn all_text(document: &Html, selector_value: &str) -> Result<Vec<String>> {
    let selector = selector(selector_value)?;
    Ok(document
        .select(&selector)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty())
        .collect())
}

fn description(document: &Html) -> Result<Option<String>> {
    let paragraphs = all_text(document, "div.description-summary div.summary__content p")?;
    if !paragraphs.is_empty() {
        return Ok(Some(paragraphs.join("\n\n")));
    }
    optional_text(document, "div.description-summary div.summary__content")
}

pub fn parse_date(value: &str, now_millis: i64) -> Option<i64> {
    let cleaned = normalize_space(value);
    if cleaned.is_empty() {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();
    if lower.contains("ago") {
        let mut parts = lower.split_whitespace();
        let amount = parts.next()?.parse::<i64>().ok()?;
        if amount < 0 {
            return None;
        }
        let unit = parts.next()?;
        let unit_seconds = match unit {
            value if value.contains("sec") => 1,
            value if value.contains("min") => 60,
            value if value.contains("hour") => 60 * 60,
            value if value.contains("day") => 24 * 60 * 60,
            value if value.contains("week") => 7 * 24 * 60 * 60,
            value if value.contains("month") => 30 * 24 * 60 * 60,
            value if value.contains("year") => 365 * 24 * 60 * 60,
            _ => return None,
        };
        let seconds = amount.checked_mul(unit_seconds)?;
        return now_millis.checked_sub(Duration::from_secs(seconds as u64).as_millis() as i64);
    }
    if let Ok(date) = DateTime::parse_from_rfc3339(&cleaned) {
        return Some(date.timestamp_millis());
    }
    for format in ["%Y-%m-%d", "%b %d, %Y", "%B %d, %Y", "%m/%d/%Y", "%d/%m/%Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(&cleaned, format) {
            return Some(
                DateTime::<Utc>::from_naive_utc_and_offset(date.and_hms_opt(0, 0, 0)?, Utc)
                    .timestamp_millis(),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestConfig;

    impl MadaraNovelConfig for TestConfig {
        const NAME: &'static str = "ClickNovel";
        const BASE_URL: &'static str = "https://clicknovel.net";
        const LANG: &'static str = "en";
    }

    fn source() -> MadaraNovelSource<TestConfig> {
        MadaraNovelSource::default()
    }

    #[test]
    fn constructs_listing_search_and_page_urls() {
        let source = source();
        assert_eq!(
            source.listing_url(2, 3),
            "https://clicknovel.net/novel/page/2/?m_orderby=trending"
        );
        let search = source.search_url("red moon", 3).unwrap();
        assert!(search.contains("s=red+moon"));
        assert!(search.contains("post_type=wp-manga"));
        assert!(search.contains("paged=3"));
    }

    #[test]
    fn parses_list_and_real_pagination_state() {
        let page = source()
            .parse_list_html(include_str!("../fixtures/list.html"), false)
            .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].title, "First Book");
        assert_eq!(
            page.entries[0].key,
            "https://clicknovel.net/novel/first-book/"
        );
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://clicknovel.net/covers/first.jpg")
        );
        assert!(page.has_next_page);
        let terminal = include_str!("../fixtures/list.html").replace(
            "<div class=\"nav-previous\"><a href=\"/novel/page/3/\">Older posts</a></div>",
            "",
        );
        assert!(
            !source()
                .parse_list_html(&terminal, false)
                .unwrap()
                .has_next_page
        );
    }

    #[test]
    fn parses_details_and_rejects_malformed_detail_pages() {
        let item = source()
            .parse_details_html(
                include_str!("../fixtures/details.html"),
                "/novel/first-book/",
            )
            .unwrap();
        assert_eq!(item.title, "First Book");
        assert_eq!(item.authors, ["Jane Author"]);
        assert_eq!(item.tags, ["Fantasy", "Adventure"]);
        assert_eq!(item.rating, Some(4.7));
        assert_eq!(item.status, Some(json!("completed")));
        assert!(source()
            .parse_details_html("<html></html>", "/bad/")
            .is_err());
    }

    #[test]
    fn parses_chapters_in_reader_order_and_dates() {
        let now = 1_700_000_000_000;
        let chapters = source()
            .parse_chapters_html(include_str!("../fixtures/chapters.html"), now)
            .unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        assert_eq!(chapters[0].date_uploaded, Some(1_705_276_800_000));
        assert_eq!(chapters[1].date_uploaded, Some(now - 7_200_000));
        assert_eq!(chapters[1].source_order, Some(1));
    }

    #[test]
    fn preserves_admin_ajax_then_ajax_then_page_fallback_order() {
        let source = source();
        assert_eq!(
            source.chapter_endpoint_urls(),
            [
                "https://clicknovel.net/wp-admin/admin-ajax.php",
                "https://clicknovel.net/ajax/chapters/",
            ]
        );
        let chapters = source
            .parse_chapter_candidates(
                [
                    "<html></html>",
                    "<html><ul></ul></html>",
                    include_str!("../fixtures/chapters.html"),
                ],
                1_700_000_000_000,
            )
            .unwrap();
        assert_eq!(chapters.len(), 2);
    }

    #[test]
    fn parses_clean_text_and_rejects_empty_chapter_pages() {
        let text = source()
            .parse_text_html(
                include_str!("../fixtures/text.html"),
                "/novel/first-book/chapter-1/",
            )
            .unwrap();
        assert_eq!(text.title.as_deref(), Some("Chapter 1: Arrival"));
        assert!(text
            .text
            .as_deref()
            .unwrap()
            .contains("The second paragraph."));
        assert!(!text
            .text
            .as_deref()
            .unwrap()
            .contains("Read latest Chapters at"));
        assert_eq!(text.blocks.len(), 3);
        assert!(source()
            .parse_text_html("<html></html>", "/empty/")
            .is_err());
    }

    #[test]
    fn exposes_title_and_all_upstream_sort_filters() {
        let filters = source().filters().unwrap();
        assert!(matches!(filters[0], FilterDefinition::Text { ref id, .. } if id == "title"));
        assert!(
            matches!(filters[1], FilterDefinition::Sort { ref options, .. } if options.len() == 6)
        );
        assert_eq!(
            MadaraNovelSource::<TestConfig>::sort_index(&json!({"sort": {"index": 4}})),
            4
        );
    }

    #[test]
    fn handles_item_and_chapter_urls_without_claiming_other_hosts() {
        let mut source = source();
        let item = source
            .handle_url("https://clicknovel.net/novel/first-book/")
            .unwrap()
            .unwrap();
        assert!(item.item.is_some());
        let chapter = source
            .handle_url("https://clicknovel.net/novel/first-book/chapter-1/")
            .unwrap()
            .unwrap();
        assert!(chapter.novel_chapter.is_some());
        assert!(source
            .handle_url("https://example.com/novel/first-book/")
            .unwrap()
            .is_none());
    }

    #[test]
    fn dispatches_ireader_fetch_commands_and_validates_inputs() {
        let mut source = source();
        let commands = source
            .dispatch("commands.describe", &json!({}))
            .unwrap()
            .unwrap();
        assert_eq!(commands["commands"].as_array().unwrap().len(), 3);
        let details = source
            .dispatch(
                "commands.detail.fetch",
                &json!({"url": "https://clicknovel.net/novel/first-book/", "html": include_str!("../fixtures/details.html")}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(details["title"], "First Book");
        let chapters = source
            .dispatch(
                "commands.chapter.fetch",
                &json!({"html": include_str!("../fixtures/chapters.html"), "nowMillis": 1_700_000_000_000_i64}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(chapters.as_array().unwrap().len(), 2);
        let text = source
            .dispatch(
                "commands.content.fetch",
                &json!({"url": "https://clicknovel.net/novel/first-book/chapter-1/", "html": include_str!("../fixtures/text.html")}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(text["title"], "Chapter 1: Arrival");
        assert!(source
            .dispatch("commands.detail.fetch", &json!({}))
            .is_err());
        assert!(source
            .dispatch("unknown.operation", &json!({}))
            .unwrap()
            .is_none());
    }
}
