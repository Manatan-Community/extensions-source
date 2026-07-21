use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, ImageRequest, NovelChapter, NovelContentBlock, NovelText, Paged,
        UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::{Captures, Regex};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;
use zip::ZipArchive;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "cyrisia";
const BASE_URL: &str = "https://cyrisia.com";
const PAGE_SIZE: usize = 36;
const MAX_EPUB_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
struct ShelfEntry {
    name: String,
    #[serde(default)]
    epubs: Vec<String>,
    #[serde(default)]
    cover: Option<String>,
}

pub struct CyrisiaSource {
    client: Client,
}

impl Default for CyrisiaSource {
    fn default() -> Self {
        Self {
            client: Client::browser(),
        }
    }
}

impl CyrisiaSource {
    fn shelf(&self) -> Result<Vec<ShelfEntry>> {
        self.client
            .get(format!("{BASE_URL}/api/bookshelf"))
            .send()?
            .error_for_status()?
            .json()
    }

    fn item(entry: &ShelfEntry) -> Result<CatalogItem> {
        let url = series_url(&entry.name)?;
        let mut item = CatalogItem::new(url.clone(), entry.name.clone());
        item.url = Some(url);
        item.language = Some("en".into());
        item.content_rating = Some("adult".into());
        item.extra.insert("epubs".into(), json!(entry.epubs));
        if let Some(cover) = entry.cover.as_deref().filter(|value| !value.is_empty()) {
            item.cover = Some(image(
                &absolute_url(BASE_URL, cover)?,
                &series_url(&entry.name)?,
            ));
        }
        Ok(item)
    }

    fn page(entries: Vec<CatalogItem>, page: u32) -> Paged<CatalogItem> {
        let page = page.max(1) as usize;
        let start = (page - 1) * PAGE_SIZE;
        let total = entries.len();
        Paged::new(
            entries.into_iter().skip(start).take(PAGE_SIZE).collect(),
            start + PAGE_SIZE < total,
        )
    }

    fn find_entry(&self, item: &CatalogItem) -> Result<ShelfEntry> {
        let title = if !item.title.trim().is_empty() {
            item.title.trim().to_owned()
        } else {
            title_from_url(item.url.as_deref().unwrap_or(&item.key))?
        };
        self.shelf()?
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(&title))
            .ok_or_else(|| Error::new(format!("Cyrisia bookshelf no longer contains {title:?}")))
    }

    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((
            html::document(response.text()?),
            response.final_url().to_owned(),
        ))
    }

    fn parse_details(document: &Html, entry: &ShelfEntry, page_url: &str) -> Result<CatalogItem> {
        let title = first_meta(document, "meta[property='og:title']")?
            .or(first_text(document, ".stitle")?)
            .unwrap_or_else(|| entry.name.clone());
        let description = first_text(document, ".synopsis-full")?
            .or(first_text(document, ".synopsis-trunc")?)
            .or(first_text(document, "[id^='syn-']")?)
            .or(first_meta(document, "meta[property='og:description']")?);
        let chips = selector(".meta-chip")?;
        let tags = document
            .select(&chips)
            .map(html::text)
            .map(|value| normalize_space(&value))
            .filter(|value| !value.is_empty())
            .collect();
        let cover = first_meta(document, "meta[property='og:image']")?
            .map(|value| absolute_url(BASE_URL, &value))
            .transpose()?;
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.into());
        item.description = description;
        item.tags = tags;
        item.cover = cover.map(|value| image(&value, page_url)).or_else(|| {
            entry
                .cover
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(|cover| {
                    image(
                        &absolute_url(BASE_URL, cover).unwrap_or_else(|_| cover.to_owned()),
                        page_url,
                    )
                })
        });
        item.initialized = true;
        item.language = Some("en".into());
        item.content_rating = Some("adult".into());
        item.extra.insert("epubs".into(), json!(entry.epubs));
        Ok(item)
    }

    fn chapter(entry: &ShelfEntry, epub: &str, index: usize) -> Result<NovelChapter> {
        let epub_url = epub_url(&entry.name, epub)?;
        Ok(NovelChapter {
            key: epub_url.clone(),
            title: Some(volume_title(epub)),
            chapter_number: Some((index + 1) as f32),
            volume_number: Some((index + 1) as f32),
            url: Some(epub_url),
            language: Some("en".into()),
            source_order: Some(index as i32),
            ..NovelChapter::default()
        })
    }

    fn download_epub(&self, url: &str) -> Result<Vec<u8>> {
        let referer = url.replace("/bibi-bookshelf/", "/read/");
        Ok(self
            .client
            .get(url)
            .header("Referer", referer)
            .max_body_bytes(MAX_EPUB_BYTES)
            .timeout_ms(60_000)
            .send()?
            .error_for_status()?
            .bytes()
            .to_vec())
    }
}

impl NovelSource for CyrisiaSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let entries = self
            .shelf()?
            .iter()
            .map(Self::item)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::page(entries, page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let mut entries = self.shelf()?;
        entries.reverse();
        Ok(Self::page(
            entries.iter().map(Self::item).collect::<Result<Vec<_>>>()?,
            page,
        ))
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim().to_ascii_lowercase();
        let entries = self
            .shelf()?
            .into_iter()
            .filter(|entry| entry.name.to_ascii_lowercase().contains(&query))
            .map(|entry| Self::item(&entry))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::page(entries, page))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let entry = self.find_entry(&item)?;
        let url = series_url(&entry.name)?;
        match self.document(&url) {
            Ok((document, final_url)) => Self::parse_details(&document, &entry, &final_url),
            Err(_) => {
                let mut item = Self::item(&entry)?;
                item.initialized = true;
                Ok(item)
            }
        }
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let entry = self.find_entry(&item)?;
        require(
            (!entry.epubs.is_empty()).then_some(()),
            "Cyrisia series has no EPUB volumes",
        )?;
        entry
            .epubs
            .iter()
            .enumerate()
            .map(|(index, epub)| Self::chapter(&entry, epub, index))
            .collect()
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let bytes = self.download_epub(url)?;
        let rendered = render_epub(&bytes)?;
        Ok(NovelText {
            html: Some(rendered.clone()),
            title: chapter.title,
            base_url: Some(url.into()),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("cyrisia.com") {
            return Ok(None);
        }
        if url.path().starts_with("/series/") {
            let title = title_from_url(candidate).unwrap_or_default();
            let mut item = CatalogItem::new(candidate, title);
            item.url = Some(candidate.into());
            item.language = Some("en".into());
            return Ok(Some(UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }));
        }
        if url.path().starts_with("/read/") || url.path().starts_with("/bibi-bookshelf/") {
            let canonical = candidate.replace("/read/", "/bibi-bookshelf/");
            return Ok(Some(UrlResolveResult {
                novel_chapter: Some(NovelChapter {
                    key: canonical.clone(),
                    url: Some(canonical),
                    language: Some("en".into()),
                    ..NovelChapter::default()
                }),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn render_epub(bytes: &[u8]) -> Result<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| Error::new(format!("invalid EPUB archive: {error}")))?;
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| Error::new(error.to_string()))?;
        if file.is_dir() || file.size() > MAX_EPUB_BYTES {
            continue;
        }
        let name = normalize_archive_path(file.name())?;
        let mut value = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut value)
            .map_err(|error| Error::new(error.to_string()))?;
        files.insert(name, value);
    }
    let container = String::from_utf8_lossy(require(
        files.get("META-INF/container.xml"),
        "EPUB has no container.xml",
    )?);
    let rootfile = capture_attr(
        &container,
        r#"(?is)<rootfile\b[^>]*full-path=[\"']([^\"']+)[\"']"#,
        1,
    )?;
    let rootfile = normalize_archive_path(&rootfile)?;
    let opf = String::from_utf8_lossy(require(
        files.get(&rootfile),
        "EPUB package document is missing",
    )?);
    let base = Path::new(&rootfile).parent().unwrap_or(Path::new(""));
    let item_re =
        Regex::new(r#"(?is)<item\b([^>]*)>"#).map_err(|error| Error::new(error.to_string()))?;
    let mut manifest = BTreeMap::<String, (String, String)>::new();
    for item in item_re.captures_iter(&opf) {
        let attrs = &item[1];
        let Some(id) = attr_value(attrs, "id") else {
            continue;
        };
        let Some(href) = attr_value(attrs, "href") else {
            continue;
        };
        manifest.insert(
            id,
            (
                resolve_archive_path(base, &href)?,
                attr_value(attrs, "media-type").unwrap_or_default(),
            ),
        );
    }
    let spine_re = Regex::new(r#"(?is)<itemref\b[^>]*idref=[\"']([^\"']+)[\"'][^>]*>"#)
        .map_err(|error| Error::new(error.to_string()))?;
    let mut output = String::new();
    for (position, spine) in spine_re.captures_iter(&opf).enumerate() {
        let Some((path, _)) = manifest.get(&spine[1]) else {
            continue;
        };
        let Some(body) = files.get(path) else {
            continue;
        };
        let mut chapter = String::from_utf8_lossy(body).into_owned();
        if should_skip_document(path, &chapter, position) {
            continue;
        }
        chapter = extract_body(&chapter).unwrap_or(chapter);
        chapter = rewrite_epub_images(
            &chapter,
            Path::new(path).parent().unwrap_or(Path::new("")),
            &files,
            &manifest,
        )?;
        chapter = sanitize_epub_html(&chapter)?;
        if !normalize_space(&html::text(html::document(&chapter).root_element())).is_empty()
            || chapter.contains("data:image/")
        {
            output.push_str("<section class=\"manatan-epub-section\">");
            output.push_str(&chapter);
            output.push_str("</section>\n");
        }
    }
    require(
        (!output.trim().is_empty()).then_some(output),
        "EPUB contains no readable spine content",
    )
}

fn rewrite_epub_images(
    html: &str,
    base: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    manifest: &BTreeMap<String, (String, String)>,
) -> Result<String> {
    let pattern = Regex::new(r#"(?i)(src|href|xlink:href)=([\"'])([^\"'#][^\"']*)[\"']"#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(pattern
        .replace_all(html, |captures: &Captures<'_>| {
            let candidate = captures[3].split('?').next().unwrap_or(&captures[3]);
            let Ok(path) = resolve_archive_path(base, candidate) else {
                return captures[0].to_owned();
            };
            let Some(bytes) = files.get(&path) else {
                return captures[0].to_owned();
            };
            let mime = manifest
                .values()
                .find(|(value, _)| value == &path)
                .map(|(_, mime)| mime.as_str())
                .filter(|value| value.starts_with("image/"))
                .unwrap_or_else(|| image_mime(&path));
            if !mime.starts_with("image/") {
                return captures[0].to_owned();
            }
            format!(
                "{}={}data:{};base64,{}{}",
                &captures[1],
                &captures[2],
                mime,
                BASE64.encode(bytes),
                &captures[2]
            )
        })
        .into_owned())
}

fn sanitize_epub_html(value: &str) -> Result<String> {
    let mut rendered = value.to_owned();
    for tag in [
        "script", "iframe", "object", "embed", "form", "style", "link",
    ] {
        let pattern = Regex::new(&format!(
            r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>|<{tag}\b[^>]*/?>"
        ))
        .map_err(|error| Error::new(error.to_string()))?;
        rendered = pattern.replace_all(&rendered, "").into_owned();
    }
    let event = Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:\"[^\"]*\"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    rendered = event.replace_all(&rendered, "").into_owned();
    let remote_double = Regex::new(
        r#"(?i)\s+(src|href|xlink:href)=\"\s*(?:https?:|javascript:|data:text/html)[^\"]*\""#,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    rendered = remote_double.replace_all(&rendered, "").into_owned();
    let remote_single = Regex::new(
        r#"(?i)\s+(src|href|xlink:href)='\s*(?:https?:|javascript:|data:text/html)[^']*'"#,
    )
    .map_err(|error| Error::new(error.to_string()))?;
    Ok(remote_single.replace_all(&rendered, "").into_owned())
}

fn normalize_archive_path(value: &str) -> Result<String> {
    let path = Path::new(value.trim_start_matches('/'));
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            _ => return Err(Error::new("EPUB archive path escapes its root")),
        }
    }
    Ok(clean.to_string_lossy().replace('\\', "/"))
}

fn resolve_archive_path(base: &Path, candidate: &str) -> Result<String> {
    let mut parts = base
        .components()
        .filter_map(|part| match part {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for part in Path::new(candidate).components() {
        match part {
            Component::Normal(value) => parts.push(value.to_owned()),
            Component::ParentDir => {
                require(parts.pop(), "EPUB reference escapes its root")?;
            }
            Component::CurDir => {}
            _ => return Err(Error::new("invalid absolute EPUB reference")),
        }
    }
    Ok(parts
        .into_iter()
        .collect::<PathBuf>()
        .to_string_lossy()
        .replace('\\', "/"))
}

fn extract_body(value: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<body\b[^>]*>(.*)</body\s*>").ok()?;
    re.captures(value).map(|capture| capture[1].to_owned())
}
fn capture_attr(value: &str, pattern: &str, group: usize) -> Result<String> {
    let re = Regex::new(pattern).map_err(|error| Error::new(error.to_string()))?;
    re.captures(value)
        .and_then(|capture| capture.get(group))
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| Error::new("required EPUB metadata is missing"))
}
fn attr_value(value: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?i)\b{}\s*=\s*[\"']([^\"']*)[\"']"#,
        regex::escape(name)
    ))
    .ok()?;
    re.captures(value).map(|capture| capture[1].to_owned())
}
fn should_skip_document(path: &str, html: &str, position: usize) -> bool {
    let value = format!("{path} {html}").to_ascii_lowercase();
    (position == 0 && (value.contains("cover") || value.contains("titlepage")))
        || ["toc", "copyright", "colophon", "about-the-publisher"]
            .iter()
            .any(|needle| path.to_ascii_lowercase().contains(needle))
}
fn image_mime(path: &str) -> &str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "image/jpeg",
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
fn first_meta(document: &Html, query: &str) -> Result<Option<String>> {
    let query = selector(query)?;
    Ok(document
        .select(&query)
        .find_map(|element| attr(element, "content")))
}
fn volume_title(value: &str) -> String {
    value
        .trim_end_matches(".epub")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn path_url(prefix: &str, parts: &[&str]) -> Result<String> {
    let mut url = Url::parse(&format!("{BASE_URL}{prefix}"))
        .map_err(|error| Error::new(error.to_string()))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::new("Cyrisia base URL cannot accept path segments"))?;
        segments.pop_if_empty();
        for part in parts {
            segments.push(part);
        }
    }
    Ok(url.to_string())
}
fn series_url(name: &str) -> Result<String> {
    path_url("/series/", &[name])
}
fn epub_url(series: &str, epub: &str) -> Result<String> {
    path_url("/bibi-bookshelf/", &[series, epub])
}
fn title_from_url(value: &str) -> Result<String> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    Ok(url
        .path_segments()
        .and_then(|mut parts| parts.nth(1))
        .unwrap_or_default()
        .replace("%20", " "))
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, CyrisiaSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn builds_encoded_public_urls() {
        assert!(series_url("A Book + More")
            .unwrap()
            .contains("A%20Book%20+%20More"));
        assert!(epub_url("A Book", "Vol 1.epub")
            .unwrap()
            .ends_with("A%20Book/Vol%201.epub"));
    }

    #[test]
    fn renders_spine_text_and_inline_images() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = SimpleFileOptions::default();
            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();
            zip.start_file("OEBPS/content.opf", options).unwrap();
            zip.write_all(br#"<package><manifest><item id="c1" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="pic" href="pic.png" media-type="image/png"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
            zip.start_file("OEBPS/chapter.xhtml", options).unwrap();
            zip.write_all(br#"<html><body><h1>Chapter</h1><p>Readable text.</p><img src="pic.png"/></body></html>"#).unwrap();
            zip.start_file("OEBPS/pic.png", options).unwrap();
            zip.write_all(b"PNG").unwrap();
            zip.finish().unwrap();
        }
        let rendered = render_epub(cursor.get_ref()).unwrap();
        assert!(rendered.contains("Readable text."));
        assert!(rendered.contains("data:image/png;base64,"));
    }
}
