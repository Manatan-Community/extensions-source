use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    marker::PhantomData,
    path::{Component, Path},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
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
use roxmltree::Document;
use serde_json::{json, Value};
use url::Url;
use zip::ZipArchive;

const RAW_ARCHIVE_PASSWORD: &[u8] = b"taiwanandnorthkorea";
const MAX_RAW_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EPUB_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;

pub trait Config: 'static {
    const BASE_URL: &'static str;
    const LANGUAGE: &'static str;
    const RAW_DOWNLOADS: bool;
}

pub struct Source<C: Config> {
    client: Client,
    config: PhantomData<C>,
    cached_epub: Option<CachedEpub>,
}

impl<C: Config> Default for Source<C> {
    fn default() -> Self {
        Self {
            client: Client::browser(),
            config: PhantomData,
            cached_epub: None,
        }
    }
}

#[derive(Clone, Debug)]
struct EpubChapter {
    path: String,
    title: String,
}

#[derive(Clone, Debug)]
struct CachedEpub {
    book_url: String,
    epub: Vec<u8>,
    chapters: Vec<EpubChapter>,
}

#[derive(Clone, Debug)]
struct RenderedEpubChapter {
    html: String,
    text: String,
}

impl<C: Config> Source<C> {
    fn response_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.client
            .get(url)
            .header("Referer", C::BASE_URL)
            .cookies_for(C::BASE_URL)
            .max_body_bytes(MAX_RAW_ARCHIVE_BYTES)
            .send()?
            .error_for_status()
            .map(|response| response.into_bytes())
    }

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

    fn raw_download_url(document: &Html) -> Result<Option<String>> {
        first_attr(document, "a.novel-download-link", "href")?
            .map(|href| absolute_url(C::BASE_URL, &href))
            .transpose()
    }

    fn download_epub(&self, book_url: &str, download_url: &str) -> Result<CachedEpub> {
        let archive = self.response_bytes(download_url)?;
        let epub = decrypt_epub_archive(&archive, RAW_ARCHIVE_PASSWORD)?;
        let chapters = parse_epub_chapters(&epub)?;
        require(
            (!chapters.is_empty()).then_some(()),
            "FuckNovelpia RAW EPUB has no readable chapters",
        )?;
        Ok(CachedEpub {
            book_url: book_url.to_owned(),
            epub,
            chapters,
        })
    }

    fn ensure_epub(&mut self, book_url: &str, download_url: &str) -> Result<&CachedEpub> {
        if self
            .cached_epub
            .as_ref()
            .is_none_or(|cached| cached.book_url != book_url)
        {
            self.cached_epub = Some(self.download_epub(book_url, download_url)?);
        }
        self.cached_epub
            .as_ref()
            .ok_or_else(|| Error::new("FuckNovelpia RAW EPUB cache is unavailable"))
    }

    fn raw_chapters(&mut self, document: &Html, book_url: &str) -> Result<Vec<NovelChapter>> {
        let download_url = require(
            Self::raw_download_url(document)?,
            "FuckNovelpia RAW download is unavailable",
        )?;
        let cached = self.ensure_epub(book_url, &download_url)?;
        Ok(cached
            .chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| NovelChapter {
                key: format!("{book_url}#epub-{}", index + 1),
                title: Some(chapter.title.clone()),
                url: Some(book_url.to_owned()),
                language: Some(C::LANGUAGE.to_owned()),
                chapter_number: Some((index + 1) as f32),
                source_order: Some(index as i32),
                extra: [
                    ("bookUrl".to_owned(), json!(book_url)),
                    ("epubPath".to_owned(), json!(chapter.path)),
                ]
                .into_iter()
                .collect(),
                ..NovelChapter::default()
            })
            .collect())
    }

    fn raw_text(&mut self, item: &CatalogItem, chapter: &NovelChapter) -> Result<NovelText> {
        let book_url = chapter
            .extra
            .get("bookUrl")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or(Self::item_url(item)?);
        if self
            .cached_epub
            .as_ref()
            .is_none_or(|cached| cached.book_url != book_url)
        {
            let (document, final_url) = self.document(&book_url)?;
            let download_url = require(
                Self::raw_download_url(&document)?,
                "FuckNovelpia RAW download is unavailable",
            )?;
            self.ensure_epub(&final_url, &download_url)?;
        }
        let path = require(
            chapter
                .extra
                .get("epubPath")
                .and_then(Value::as_str)
                .map(str::to_owned),
            "FuckNovelpia RAW chapter has no EPUB path",
        )?;
        let cached = require(
            self.cached_epub.as_ref(),
            "FuckNovelpia RAW EPUB cache is unavailable",
        )?;
        let chapter = require(
            cached
                .chapters
                .iter()
                .find(|candidate| candidate.path == path),
            "FuckNovelpia RAW EPUB chapter is unavailable",
        )?;
        let rendered = render_epub_chapter(&cached.epub, &chapter.path)?;
        Ok(NovelText {
            html: Some(rendered.html.clone()),
            text: Some(rendered.text),
            title: Some(chapter.title.clone()),
            base_url: Some(book_url),
            blocks: vec![NovelContentBlock::Text {
                text: rendered.html,
                html: true,
            }],
            ..NovelText::default()
        })
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
        if C::RAW_DOWNLOADS {
            return self.raw_chapters(&document, &final_url);
        }
        Self::parse_chapters(&document, &final_url)
    }

    fn text(&mut self, item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        if C::RAW_DOWNLOADS {
            return self.raw_text(&item, &chapter);
        }
        let page_url = absolute_url(C::BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
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

#[derive(Clone, Debug)]
struct ManifestItem {
    path: String,
    media_type: String,
}

fn decrypt_epub_archive(archive: &[u8], password: &[u8]) -> Result<Vec<u8>> {
    let mut archive =
        ZipArchive::new(Cursor::new(archive)).map_err(|error| Error::new(error.to_string()))?;
    let epub_index = archive
        .file_names()
        .position(|name| name.to_ascii_lowercase().ends_with(".epub"));
    let epub_index = require(epub_index, "FuckNovelpia RAW ZIP contains no EPUB")?;
    let mut file = archive
        .by_index_decrypt(epub_index, password)
        .map_err(|error| Error::new(format!("Failed to decrypt FuckNovelpia RAW ZIP: {error}")))?;
    if file.size() > MAX_RAW_ARCHIVE_BYTES {
        return Err(Error::new("FuckNovelpia RAW EPUB is too large"));
    }
    let mut epub = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut epub)
        .map_err(|error| Error::new(format!("Failed to read FuckNovelpia RAW EPUB: {error}")))?;
    Ok(epub)
}

fn parse_epub_chapters(epub: &[u8]) -> Result<Vec<EpubChapter>> {
    let container = epub_file_text(epub, "META-INF/container.xml")?;
    let container = strip_xml_doctype(&container);
    let container = Document::parse(&container)
        .map_err(|error| Error::new(format!("Invalid EPUB container: {error}")))?;
    let opf_path = require(
        container
            .descendants()
            .find(|node| node.tag_name().name() == "rootfile")
            .and_then(|node| node.attribute("full-path"))
            .map(str::to_owned),
        "EPUB container has no package path",
    )?;
    let opf = epub_file_text(epub, &opf_path)?;
    let opf = strip_xml_doctype(&opf);
    let opf = Document::parse(&opf)
        .map_err(|error| Error::new(format!("Invalid EPUB package: {error}")))?;
    let mut manifest = HashMap::new();
    for item in opf
        .descendants()
        .filter(|node| node.tag_name().name() == "item")
    {
        let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) else {
            continue;
        };
        let path = normalize_epub_path(&opf_path, href)?;
        let media_type = item.attribute("media-type").unwrap_or("").to_owned();
        manifest.insert(
            id.to_owned(),
            ManifestItem {
                path: path.clone(),
                media_type: media_type.clone(),
            },
        );
    }

    let toc_titles = epub_toc_titles(epub, &manifest, &opf)?;
    let mut chapters = Vec::new();
    for idref in opf
        .descendants()
        .filter(|node| node.tag_name().name() == "itemref")
        .filter_map(|node| node.attribute("idref"))
    {
        let Some(item) = manifest.get(idref) else {
            continue;
        };
        if !item.media_type.contains("html") {
            continue;
        }
        let title = toc_titles
            .get(&item.path)
            .cloned()
            .or_else(|| epub_chapter_title(epub, &item.path).ok().flatten())
            .unwrap_or_else(|| format!("Chapter {}", chapters.len() + 1));
        chapters.push(EpubChapter {
            path: item.path.clone(),
            title,
        });
    }
    Ok(chapters)
}

fn epub_chapter_title(epub: &[u8], chapter_path: &str) -> Result<Option<String>> {
    let contents = epub_file_text(epub, chapter_path)?;
    let document = html::document(&contents);
    for selector_value in ["head title", "body h1", "body h2"] {
        let title_selector = selector(selector_value)?;
        if let Some(title) = document
            .select(&title_selector)
            .next()
            .map(html::text)
            .map(|title| normalize_space(&title))
            .filter(|title| !title.is_empty())
        {
            return Ok(Some(title));
        }
    }
    Ok(None)
}

fn render_epub_chapter(epub: &[u8], chapter_path: &str) -> Result<RenderedEpubChapter> {
    let contents = epub_file_text(epub, chapter_path)?;
    let document = html::document(&contents);
    let body_selector = selector("body")?;
    let body = require(
        document.select(&body_selector).next(),
        format!("EPUB chapter has no body: {chapter_path}"),
    )?;
    let html = inline_epub_images(&body.inner_html(), chapter_path, epub)?;
    require(
        (!html.trim().is_empty()).then_some(()),
        format!("EPUB chapter is empty: {chapter_path}"),
    )?;
    Ok(RenderedEpubChapter {
        html,
        text: normalize_space(&html::text(body)),
    })
}

fn epub_file_bytes(epub: &[u8], path: &str) -> Result<Vec<u8>> {
    let mut archive = ZipArchive::new(Cursor::new(epub))
        .map_err(|error| Error::new(format!("Invalid EPUB ZIP: {error}")))?;
    let mut file = archive
        .by_name(path)
        .map_err(|error| Error::new(format!("EPUB file is missing ({path}): {error}")))?;
    if file.size() > MAX_EPUB_UNCOMPRESSED_BYTES {
        return Err(Error::new(format!("EPUB entry is too large: {path}")));
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| Error::new(format!("Failed to extract EPUB entry {path}: {error}")))?;
    Ok(bytes)
}

fn epub_file_text(epub: &[u8], path: &str) -> Result<String> {
    String::from_utf8(epub_file_bytes(epub, path)?)
        .map_err(|error| Error::new(format!("EPUB file is not UTF-8 ({path}): {error}")))
}

fn epub_toc_titles(
    epub: &[u8],
    manifest: &HashMap<String, ManifestItem>,
    opf: &Document<'_>,
) -> Result<HashMap<String, String>> {
    let toc_id = opf
        .descendants()
        .find(|node| node.tag_name().name() == "spine")
        .and_then(|node| node.attribute("toc"));
    let toc_item = toc_id.and_then(|id| manifest.get(id)).or_else(|| {
        manifest
            .values()
            .find(|item| item.media_type == "application/x-dtbncx+xml")
    });
    let Some(toc_item) = toc_item else {
        return Ok(HashMap::new());
    };
    let toc = epub_file_text(epub, &toc_item.path)?;
    let toc = strip_xml_doctype(&toc);
    let toc = Document::parse(&toc)
        .map_err(|error| Error::new(format!("Invalid EPUB navigation: {error}")))?;
    let mut titles = HashMap::new();
    for nav_point in toc
        .descendants()
        .filter(|node| node.tag_name().name() == "navPoint")
    {
        let title = nav_point
            .descendants()
            .find(|node| node.tag_name().name() == "navLabel")
            .and_then(|label| {
                label
                    .descendants()
                    .find(|node| node.tag_name().name() == "text")
            })
            .and_then(|node| node.text())
            .map(normalize_space)
            .filter(|value| !value.is_empty());
        let src = nav_point
            .descendants()
            .find(|node| node.tag_name().name() == "content")
            .and_then(|node| node.attribute("src"));
        if let (Some(title), Some(src)) = (title, src) {
            titles.insert(normalize_epub_path(&toc_item.path, src)?, title);
        }
    }
    Ok(titles)
}

fn inline_epub_images(body_html: &str, chapter_path: &str, epub: &[u8]) -> Result<String> {
    let source = Regex::new(r#"(?i)(src|xlink:href)\s*=\s*(["'])([^"']+)(["'])"#)
        .map_err(|error| Error::new(error.to_string()))?;
    let mut images = HashMap::new();
    for captures in source.captures_iter(body_html) {
        let Some(candidate) = captures.get(3).map(|value| value.as_str()) else {
            continue;
        };
        if candidate.starts_with("data:")
            || candidate.starts_with("http://")
            || candidate.starts_with("https://")
        {
            continue;
        }
        let Ok(path) = normalize_epub_path(chapter_path, candidate) else {
            continue;
        };
        let Ok(bytes) = epub_file_bytes(epub, &path) else {
            continue;
        };
        images.insert(
            candidate.to_owned(),
            format!(
                "data:{};base64,{}",
                media_type_from_path(&path),
                BASE64_STANDARD.encode(bytes)
            ),
        );
    }
    Ok(source
        .replace_all(body_html, |captures: &regex::Captures<'_>| {
            let candidate = captures.get(3).map(|value| value.as_str()).unwrap_or("");
            let Some(data_url) = images.get(candidate) else {
                return captures[0].to_owned();
            };
            format!(
                "{}={}{}{}",
                &captures[1], &captures[2], data_url, &captures[4]
            )
        })
        .into_owned())
}

fn normalize_epub_path(base_file: &str, candidate: &str) -> Result<String> {
    let candidate = candidate
        .split(['#', '?'])
        .next()
        .unwrap_or(candidate)
        .trim_start_matches('/');
    let joined = Path::new(base_file)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(candidate);
    let mut parts = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(Error::new("EPUB resource escapes the archive root"));
                }
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                return Err(Error::new("EPUB resource path is invalid"));
            }
        }
    }
    Ok(parts.join("/"))
}

fn strip_xml_doctype(value: &str) -> String {
    let Some(start) = value.find("<!DOCTYPE") else {
        return value.to_owned();
    };
    let mut bracket_depth = 0_u32;
    let mut end = None;
    for (offset, character) in value[start..].char_indices() {
        match character {
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '>' if bracket_depth == 0 => {
                end = Some(start + offset + character.len_utf8());
                break;
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return value[..start].to_owned();
    };
    format!("{}{}", &value[..start], &value[end..])
}

fn media_type_from_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
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
    use std::io::Write;

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
    fn resolves_korean_raw_download_url() {
        let document = html::document(
            r#"<a class="novel-download-link" href="/download.php?slug=1">Download</a>"#,
        );
        assert_eq!(
            Source::<Korean>::raw_download_url(&document)
                .unwrap()
                .as_deref(),
            Some("https://raw-fucknovelpia.com/download.php?slug=1")
        );
    }

    #[test]
    fn parses_epub_spine_titles_text_and_inline_images() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (path, contents) in [
            (
                "META-INF/container.xml",
                br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#
                    .as_slice(),
            ),
            (
                "OEBPS/content.opf",
                br#"<package><manifest>
                    <item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
                    <item id="chapter" href="Text/chapter.xhtml" media-type="application/xhtml+xml"/>
                    <item id="image" href="Images/art.png" media-type="image/png"/>
                    </manifest><spine toc="toc"><itemref idref="chapter"/></spine></package>"#
                    .as_slice(),
            ),
            (
                "OEBPS/toc.ncx",
                br#"<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
                    <ncx><navMap><navPoint><navLabel><text>EPUB chapter title</text></navLabel>
                    <content src="Text/chapter.xhtml"/></navPoint></navMap></ncx>"#
                    .as_slice(),
            ),
            (
                "OEBPS/Text/chapter.xhtml",
                br#"<html><head><title>Fallback</title></head><body>
                    <h1>Heading</h1><p>Hello EPUB.</p><img src="../Images/art.png"/>
                    </body></html>"#
                    .as_slice(),
            ),
            ("OEBPS/Images/art.png", b"png".as_slice()),
        ] {
            writer.start_file(path, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        let epub = writer.finish().unwrap().into_inner();

        let chapters = parse_epub_chapters(&epub).unwrap();
        let rendered = render_epub_chapter(&epub, &chapters[0].path).unwrap();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].title, "EPUB chapter title");
        assert!(rendered.text.contains("Hello EPUB."));
        assert!(rendered.html.contains("src=\"data:image/png;base64,cG5n\""));
    }

    #[test]
    fn decrypts_raw_archive_with_shared_password() {
        const ENCRYPTED_FIXTURE: &str = "UEsDBBQACQAIAHl0/VwMkwwrMgIAAMoEAAAWABwAY29kZXgtZm5wLWZpeHR1cmUuZXB1YlVUCQADJWRqaiVkamp1eAsAAQT1AQAABAAAAACvK7vc3JXgfLoEuvd5xhgPQH7iOS9L82NAImOZ3QCAF4MhanKI7VNYbr/vdNxhBO57POEYEhihJUNd+24hAdo0bO/FQJXRMSpCCaJvYmb1vVpmjkDXIXRxqn3kxCtV9jCwf5EsczjlDVHnEGaWMjinXovFEF0rdewVeXowtC10ADDNRNAcK0T4umImQ6SM0ARsF6eiDZKXmMpMD16c7ybWT+egxk/D2Jbje0X77izje83jiYQ1gArrTkvdX0LKB6Bdl6wThfUoa7zlD2ZBKdx4bLFqps2oTKasqgDrPj7ZRwBpRVIgob6dYKmBMflmf5KguU8khdpmFAM1+qEHxLF67yks1a9S37FnsXsQbTDFlGC/OFU0M+tko+yJu+EsBnTi2RVXMlzFWJ2OSYbBTkQXZQ6WeNVcjJrwmqtc5RCQWdi8h4bNiHh40aK/xCdXgUpCuJBaeOotPM4Wxw82A+AMefSbi2H1sksOj0rfHxd9mDEpeQtvGXsjesB+VHNoHdHbmdZ0ov3xY4F8NziQTt0B1ZYOcKiSzVpIErspae7YyTJS1DzcJWC70MTlTYZc4s3wmNOjVBnEhkEU9re6iAgUrfIKAoZmIvMkY1UFJCbl+7lbSq3uHbrbabvTvAxU8OO79gRr8i4Hxu2dTc5DbIPULrYSv5q/P5UIlUId2DFrhJuGXxYmCJsL764Jt6PCqtKqFXFs3bNSNNIVgzhsx+AGXjzIvF5EbFMuKai9Ies6WIQgUEsHCAyTDCsyAgAAygQAAFBLAQIeAxQACQAIAHl0/VwMkwwrMgIAAMoEAAAWABgAAAAAAAAAAACkgQAAAABjb2RleC1mbnAtZml4dHVyZS5lcHViVVQFAAMlZGpqdXgLAAEE9QEAAAQAAAAAUEsFBgAAAAABAAEAXAAAAJICAAAAAA==";
        let archive = BASE64_STANDARD.decode(ENCRYPTED_FIXTURE).unwrap();
        let epub = decrypt_epub_archive(&archive, RAW_ARCHIVE_PASSWORD).unwrap();
        let chapters = parse_epub_chapters(&epub).unwrap();
        let rendered = render_epub_chapter(&epub, &chapters[0].path).unwrap();

        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].title, "Fixture chapter");
        assert!(rendered.text.contains("Decrypted EPUB text."));
        assert!(decrypt_epub_archive(&archive, b"wrong-password").is_err());
    }
}
