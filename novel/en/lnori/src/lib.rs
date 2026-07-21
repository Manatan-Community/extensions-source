// Ported from the observable behavior of LNReader's MIT-licensed LNORI source.

use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, ElementRef, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "lnori";
const BASE_URL: &str = "https://lnori.com";
const PAGE_SIZE: usize = 36;

pub struct LnoriSource {
    client: Client,
}

impl Default for LnoriSource {
    fn default() -> Self {
        Self {
            client: Client::browser(),
        }
    }
}

#[derive(Clone)]
struct LibraryEntry {
    item: CatalogItem,
    author: String,
    tags: Vec<String>,
}

impl LnoriSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn library(&self) -> Result<Vec<LibraryEntry>> {
        let (document, _) = self.document(&format!("{BASE_URL}/library"))?;
        Self::parse_library(&document)
    }

    fn parse_library(document: &Html) -> Result<Vec<LibraryEntry>> {
        let cards = selector("article.card")?;
        let links = selector("a.stretched-link")?;
        let covers = selector(".card-cover img")?;
        let mut entries = Vec::new();
        for card in document.select(&cards) {
            let Some(title) = attr(card, "data-t") else {
                continue;
            };
            let Some(href) = card
                .select(&links)
                .find_map(|element| attr(element, "href"))
            else {
                continue;
            };
            let url = absolute_url(BASE_URL, &href)?;
            let author = attr(card, "data-a").unwrap_or_default();
            let tags = attr(card, "data-tags")
                .unwrap_or_default()
                .split(',')
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            let mut item = CatalogItem::new(url.clone(), title);
            item.url = Some(url);
            item.authors = (!author.is_empty())
                .then_some(author.clone())
                .into_iter()
                .collect();
            item.tags = tags.clone();
            item.language = Some("en".into());
            item.content_rating = Some("safe".into());
            if let Some(src) = card
                .select(&covers)
                .find_map(|element| attr(element, "src"))
            {
                item.cover = Some(image(&absolute_url(BASE_URL, &src)?, BASE_URL));
            }
            entries.push(LibraryEntry { item, author, tags });
        }
        Ok(entries)
    }

    fn page(entries: Vec<CatalogItem>, page: u32) -> Paged<CatalogItem> {
        let page = page.max(1) as usize;
        let start = (page - 1) * PAGE_SIZE;
        let total = entries.len();
        let values = entries.into_iter().skip(start).take(PAGE_SIZE).collect();
        Paged::new(values, start + PAGE_SIZE < total)
    }

    fn item_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let mut url = Url::parse(&absolute_url(BASE_URL, candidate)?)
            .map_err(|error| Error::new(error.to_string()))?;
        url.set_fragment(None);
        Ok(url.to_string())
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = first_text(document, ".hero-card h1.s-title")?
            .or(first_meta(document, "meta[property='og:title']")?)
            .ok_or_else(|| Error::new("LNORI series has no title"))?;
        let author = first_text(document, ".hero-card p.author")?.unwrap_or_default();
        let mut descriptions = Vec::new();
        let description_nodes = selector("section.desc-box p.description")?;
        for node in document.select(&description_nodes) {
            let value = normalize_space(&html::text(node));
            if !value.is_empty() {
                descriptions.push(value);
            }
        }
        let description = if descriptions.is_empty() {
            first_meta(document, "meta[property='og:description']")?
        } else {
            Some(descriptions.join("\n\n"))
        };
        let tags = parse_tags(document)?;
        let cover = first_attr(document, ".hero-card .cover-wrap img", "src")?
            .or(first_meta(document, "meta[property='og:image']")?)
            .map(|src| absolute_url(BASE_URL, &src))
            .transpose()?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.to_owned());
        item.authors = (!author.is_empty()).then_some(author).into_iter().collect();
        item.tags = tags;
        item.description = description;
        item.cover = cover.map(|value| image(&value, page_url));
        item.initialized = true;
        item.language = Some("en".into());
        item.content_rating = Some("safe".into());
        Ok(item)
    }

    fn volume_links(document: &Html) -> Result<Vec<(String, String)>> {
        let links = selector("a[href^='/book/']")?;
        let mut values = Vec::<(String, String)>::new();
        for anchor in document.select(&links) {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let url = absolute_url(BASE_URL, &href)?;
            let label = normalize_space(&html::text(anchor))
                .replace("Start Reading", "")
                .trim()
                .to_owned();
            if let Some(existing) = values.iter_mut().find(|(value, _)| value == &url) {
                if label.len() > existing.1.len() {
                    existing.1 = label;
                }
            } else {
                values.push((url, label));
            }
        }
        Ok(values)
    }

    fn parse_volume(
        document: &Html,
        volume_url: &str,
        label: &str,
        offset: usize,
    ) -> Result<Vec<NovelChapter>> {
        let toc = selector("nav.toc-view a[href^='#'], nav#toc-list a[href^='#']")?;
        let sections = selector("section.chapter[id]")?;
        let mut anchors = Vec::<(String, String)>::new();
        for anchor in document.select(&toc) {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let id = href.trim_start_matches('#').to_owned();
            if !id.is_empty() {
                anchors.push((id, normalize_space(&html::text(anchor))));
            }
        }
        if anchors.is_empty() {
            for section in document.select(&sections) {
                let Some(id) = attr(section, "id") else {
                    continue;
                };
                let title = first_text_in(section, "h2.chapter-title, h2, h3")?
                    .unwrap_or_else(|| format!("Section {}", anchors.len() + 1));
                anchors.push((id, title));
            }
        }
        let volume = if label.is_empty() {
            Url::parse(volume_url)
                .ok()
                .and_then(|url| url.path_segments()?.next_back().map(slug_title))
                .unwrap_or_else(|| "Volume".into())
        } else {
            label.to_owned()
        };
        Ok(anchors
            .into_iter()
            .enumerate()
            .map(|(index, (id, title))| {
                let key = format!("{volume_url}#{id}");
                NovelChapter {
                    key: key.clone(),
                    title: Some(format!("{volume} - {title}")),
                    chapter_number: Some((offset + index + 1) as f32),
                    url: Some(key),
                    language: Some("en".into()),
                    source_order: Some((offset + index) as i32),
                    ..NovelChapter::default()
                }
            })
            .collect())
    }

    fn parse_text(document: &Html, chapter_url: &str) -> Result<NovelText> {
        let parsed = Url::parse(chapter_url).map_err(|error| Error::new(error.to_string()))?;
        let anchor = parsed
            .fragment()
            .ok_or_else(|| Error::new("LNORI chapter URL has no section anchor"))?;
        let query = format!("section#{}", css_identifier(anchor));
        let section_selector = selector(&query)?;
        let section = document
            .select(&section_selector)
            .next()
            .ok_or_else(|| Error::new(format!("LNORI chapter section {anchor:?} was not found")))?;
        let toc = selector("nav.toc-view a[href^='#'], nav#toc-list a[href^='#']")?;
        let ids = document
            .select(&toc)
            .filter_map(|node| attr(node, "href"))
            .map(|value| value.trim_start_matches('#').to_owned())
            .collect::<Vec<_>>();
        let next_id = ids
            .iter()
            .position(|value| value == anchor)
            .and_then(|index| ids.get(index + 1));
        let mut html_parts = Vec::new();
        let mut current = Some(section);
        while let Some(node) = current {
            if node.value().name() == "section"
                && attr(node, "id").as_deref() == next_id.map(String::as_str)
            {
                break;
            }
            if node.value().name() == "section" && has_class(node, "chapter") {
                let main = first_element(node, ".main")?.unwrap_or(node);
                html_parts.push(sanitize_html(&main.inner_html(), &format!("{BASE_URL}/"))?);
            }
            current = next_element_sibling(node);
        }
        let rendered = html_parts.join("\n");
        require(
            (!rendered.trim().is_empty()).then_some(()),
            "LNORI chapter has no readable text",
        )?;
        let base = format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or("lnori.com"),
            parsed.path()
        );
        Ok(NovelText {
            html: Some(rendered.clone()),
            base_url: Some(base.clone()),
            image_context: Some(ImageRequestContext {
                headers: [("Referer".into(), base)].into_iter().collect(),
                cookie_url: None,
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }
}

impl NovelSource for LnoriSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing("popular", page, &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if listing != "popular" {
            return Err(Error::new(format!("unknown novel listing {listing:?}")));
        }
        let mut entries = self.library()?;
        if let Some(genre) = filters
            .get("genre")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            entries.retain(|entry| entry.tags.iter().any(|tag| tag == genre));
        }
        match filters
            .get("sort")
            .and_then(Value::as_str)
            .unwrap_or("popular")
        {
            "title-az" => entries.sort_by(|a, b| {
                a.item
                    .title
                    .to_lowercase()
                    .cmp(&b.item.title.to_lowercase())
            }),
            "title-za" => entries.sort_by(|a, b| {
                b.item
                    .title
                    .to_lowercase()
                    .cmp(&a.item.title.to_lowercase())
            }),
            _ => {}
        }
        Ok(Self::page(
            entries.into_iter().map(|entry| entry.item).collect(),
            page,
        ))
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim().to_ascii_lowercase();
        let entries = self
            .library()?
            .into_iter()
            .filter(|entry| {
                entry.item.title.to_ascii_lowercase().contains(&query)
                    || entry.author.to_ascii_lowercase().contains(&query)
                    || entry.tags.iter().any(|tag| tag.contains(&query))
            })
            .map(|entry| entry.item)
            .collect();
        Ok(Self::page(entries, page))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::item_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = Self::item_url(&item)?;
        let (document, _) = self.document(&url)?;
        let mut chapters = Vec::new();
        for (volume_url, label) in Self::volume_links(&document)? {
            let (volume, _) = self.document(&volume_url)?;
            chapters.extend(Self::parse_volume(
                &volume,
                &volume_url,
                &label,
                chapters.len(),
            )?);
        }
        require(
            (!chapters.is_empty()).then_some(()),
            "LNORI series has no readable chapters",
        )?;
        Ok(chapters)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let parsed = Url::parse(&absolute_url(BASE_URL, url)?)
            .map_err(|error| Error::new(error.to_string()))?;
        let mut page = parsed.clone();
        page.set_fragment(None);
        let (document, _) = self.document(page.as_str())?;
        Self::parse_text(&document, parsed.as_str())
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter(
                "sort",
                "Sort By",
                &[
                    ("Popular (Default)", "popular"),
                    ("Title A-Z", "title-az"),
                    ("Title Z-A", "title-za"),
                ],
            ),
            select_filter("genre", "Genre", GENRES),
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("lnori.com") {
            return Ok(None);
        }
        let path = url.path();
        if path.starts_with("/series/") {
            let mut item = CatalogItem::new(candidate, "");
            item.url = Some(candidate.to_owned());
            item.language = Some("en".into());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        if path.starts_with("/book/") {
            let chapter = url.fragment().map(|_| NovelChapter {
                key: candidate.into(),
                url: Some(candidate.into()),
                language: Some("en".into()),
                ..NovelChapter::default()
            });
            return Ok(Some(UrlResolveResult {
                novel_chapter: chapter,
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url).header("Referer", referer)
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

fn first_text_in(root: ElementRef<'_>, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(root
        .select(&query)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn first_attr(document: &Html, query: &str, name: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .find_map(|element| attr(element, name)))
}

fn first_meta(document: &Html, query: &str) -> Result<Option<String>> {
    first_attr(document, query, "content")
}

fn parse_tags(document: &Html) -> Result<Vec<String>> {
    let nav = selector("nav.tags-box")?;
    if let Some(raw) = document
        .select(&nav)
        .find_map(|node| attr(node, "data-tags"))
    {
        if let Ok(values) = serde_json::from_str::<Value>(&raw) {
            let mut tags = values
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.get("name")?.as_str())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            tags.sort();
            tags.dedup();
            if !tags.is_empty() {
                return Ok(tags);
            }
        }
    }
    let links = selector("nav.tags-box a")?;
    let mut tags = document
        .select(&links)
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

fn first_element<'a>(root: ElementRef<'a>, query: &str) -> Result<Option<ElementRef<'a>>> {
    let query = selector(query)?;
    Ok(root.select(&query).next())
}

fn next_element_sibling(node: ElementRef<'_>) -> Option<ElementRef<'_>> {
    let mut sibling = node.next_sibling();
    while let Some(value) = sibling {
        if let Some(element) = ElementRef::wrap(value) {
            return Some(element);
        }
        sibling = value.next_sibling();
    }
    None
}

fn has_class(node: ElementRef<'_>, class: &str) -> bool {
    node.value().classes().any(|value| value == class)
}

fn css_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
        .collect()
}

fn sanitize_html(value: &str, base: &str) -> Result<String> {
    let mut rendered = value.to_owned();
    for tag in ["script", "iframe", "object", "embed", "style"] {
        let pattern = regex::Regex::new(&format!(
            r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>|<{tag}\b[^>]*/?>"
        ))
        .map_err(|error| Error::new(error.to_string()))?;
        rendered = pattern.replace_all(&rendered, "").into_owned();
    }
    let event = regex::Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:\"[^\"]*\"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    rendered = event.replace_all(&rendered, "").into_owned();
    let double = regex::Regex::new(r#"(?i)(src|srcset)=\"(/[^\"]*)\""#)
        .map_err(|error| Error::new(error.to_string()))?;
    rendered = double
        .replace_all(&rendered, |captures: &regex::Captures<'_>| {
            let absolute =
                absolute_url(base, &captures[2]).unwrap_or_else(|_| captures[2].to_owned());
            format!("{}=\"{}\"", &captures[1], absolute)
        })
        .into_owned();
    let single = regex::Regex::new(r#"(?i)(src|srcset)='(/[^']*)'"#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(single
        .replace_all(&rendered, |captures: &regex::Captures<'_>| {
            let absolute =
                absolute_url(base, &captures[2]).unwrap_or_else(|_| captures[2].to_owned());
            format!("{}='{}'", &captures[1], absolute)
        })
        .into_owned())
}

fn slug_title(value: &str) -> String {
    value
        .split('-')
        .filter(|value| !value.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index: 0,
    }
}

const GENRES: &[(&str, &str)] = &[
    ("All", ""),
    ("Academy", "academy"),
    ("Action", "action"),
    ("Adventure", "adventure"),
    ("Comedy", "comedy"),
    ("Drama", "drama"),
    ("Fantasy", "fantasy"),
    ("Harem", "harem"),
    ("Historical", "historical"),
    ("Isekai", "isekai"),
    ("Magic", "magic"),
    ("Mystery", "mystery"),
    ("Psychological", "psychological"),
    ("Reincarnation", "reincarnation"),
    ("Romance", "romance"),
    ("Sci-Fi", "sci-fi"),
    ("Slice of Life", "slice-of-life"),
    ("Tragedy", "tragedy"),
    ("Female Protagonist", "female protagonist"),
    ("Male Protagonist", "male protagonist"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, LnoriSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_fixture() {
        let document = html::document(include_str!("../tests/fixtures/library.html"));
        let entries = LnoriSource::parse_library(&document).unwrap();
        assert_eq!(entries[0].item.title, "Fixture Novel");
        assert_eq!(entries[0].author, "Fixture Author");
        assert_eq!(entries[0].tags, vec!["fantasy", "action"]);
    }

    #[test]
    fn parses_details_volume_and_text_fixtures() {
        let series = html::document(include_str!("../tests/fixtures/series.html"));
        let item =
            LnoriSource::parse_details(&series, "https://lnori.com/series/1/fixture").unwrap();
        assert_eq!(item.title, "Fixture Novel");
        assert_eq!(LnoriSource::volume_links(&series).unwrap().len(), 1);
        let volume = html::document(include_str!("../tests/fixtures/book.html"));
        let chapters =
            LnoriSource::parse_volume(&volume, "https://lnori.com/book/1/fixture", "Volume One", 0)
                .unwrap();
        assert_eq!(chapters.len(), 2);
        let text = LnoriSource::parse_text(&volume, chapters[0].url.as_deref().unwrap()).unwrap();
        assert!(text.html.unwrap().contains("First paragraph"));
    }
}
