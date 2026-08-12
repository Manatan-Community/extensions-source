use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use manatan_common::require;
use manatan_sdk::{
    client::Client,
    html,
    model::{
        CatalogItem, FilterDefinition, ImageRequest, NovelChapter, NovelContentBlock, NovelText,
        OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use zip::ZipArchive;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "bookracy";
const BASE_URL: &str = "https://bookracy.com";
const API_URL: &str = "https://api.bookracy.com";
const PAGE_SIZE: usize = 50;
const MAX_EPUB_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Book {
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    md5: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    book_image: String,
    #[serde(default)]
    book_filetype: String,
    #[serde(default)]
    book_lang: String,
    #[serde(default)]
    book_size: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    publisher: String,
    #[serde(default)]
    year: String,
    #[serde(default)]
    series: String,
    #[serde(default)]
    isbn: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Book>,
}

#[derive(Deserialize)]
struct TrendingResponse {
    #[serde(default)]
    trending: Vec<Book>,
}

pub struct BookRacySource {
    client: Client,
}

impl Default for BookRacySource {
    fn default() -> Self {
        Self {
            client: Client::browser(),
        }
    }
}

impl BookRacySource {
    fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        self.client
            .get(url)
            .header("Referer", BASE_URL)
            .send()?
            .error_for_status()?
            .json()
    }

    fn search_books(&self, query: &str, page: u32, filters: &Value) -> Result<Vec<Book>> {
        let mut url = Url::parse(&format!("{API_URL}/api/books"))
            .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("query", query.trim());
            pairs.append_pair("lang", "en");
            pairs.append_pair("ext", "epub");
            pairs.append_pair("limit", &PAGE_SIZE.to_string());
            pairs.append_pair("page", &page.max(1).to_string());
            let content = filter_value(filters, "content", "all");
            if content != "all" {
                pairs.append_pair("content", content);
            }
            let sort = filter_value(filters, "sort", "relevance");
            if sort != "relevance" {
                pairs.append_pair("sort", sort);
            }
        }
        let response: SearchResponse = self.get_json(url.as_str())?;
        Ok(response
            .results
            .into_iter()
            .filter(is_readable_english_epub)
            .collect())
    }

    fn lookup(&self, md5: &str) -> Result<Book> {
        self.search_books(md5, 1, &json!({}))?
            .into_iter()
            .find(|book| book.md5.eq_ignore_ascii_case(md5))
            .ok_or_else(|| Error::new("BookRacy no longer contains this EPUB"))
    }

    fn book_from_item(&self, item: &CatalogItem) -> Result<Book> {
        if let Some(value) = item.extra.get("book") {
            let book: Book = serde_json::from_value(value.clone())
                .map_err(|error| Error::new(error.to_string()))?;
            if is_valid_md5(&book.md5) && is_readable_english_epub(&book) {
                return Ok(book);
            }
        }
        self.lookup(&item_md5(item)?)
    }

    fn item(book: &Book, initialized: bool) -> Result<CatalogItem> {
        require(
            is_valid_md5(&book.md5).then_some(()),
            "BookRacy result has no valid MD5",
        )?;
        require(
            (!book.title.trim().is_empty()).then_some(()),
            "BookRacy result has no title",
        )?;
        require(
            is_readable_english_epub(book).then_some(()),
            "BookRacy result is not a readable English EPUB",
        )?;
        let page_url = book_url(&book.md5)?;
        let mut item = CatalogItem::new(book.md5.to_ascii_lowercase(), book.title.trim());
        item.url = Some(page_url.clone());
        item.authors = nonempty(&book.author).into_iter().collect();
        item.description = nonempty(&book.description);
        item.cover = nonempty(&book.book_image)
            .map(|cover| ImageRequest::get(cover).header("Referer", BASE_URL));
        item.tags = metadata_tags(book);
        item.status = Some(json!("completed"));
        item.initialized = initialized;
        item.language = Some("en".into());
        item.content_rating = Some("adult".into());
        item.extra.insert("bookracyMd5".into(), json!(book.md5));
        item.extra.insert(
            "book".into(),
            serde_json::to_value(book).map_err(|error| Error::new(error.to_string()))?,
        );
        Ok(item)
    }

    fn chapter(book: &Book) -> Result<NovelChapter> {
        validate_download_url(&book.link)?;
        Ok(NovelChapter {
            key: book.link.clone(),
            title: Some("Full book".into()),
            chapter_number: Some(1.0),
            volume_number: Some(1.0),
            url: Some(book.link.clone()),
            language: Some("en".into()),
            source_order: Some(0),
            ..NovelChapter::default()
        })
    }

    fn download_epub(&self, url: &str) -> Result<Vec<u8>> {
        validate_download_url(url)?;
        let response = self
            .client
            .get(url)
            .header("Referer", BASE_URL)
            .max_body_bytes(MAX_EPUB_BYTES)
            .timeout_ms(60_000)
            .send()?
            .error_for_status()?;
        let bytes = response.bytes().to_vec();
        require(
            bytes.starts_with(b"PK").then_some(()),
            "BookRacy download is not an EPUB archive",
        )?;
        Ok(bytes)
    }
}

impl NovelSource for BookRacySource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        let response: TrendingResponse = self.get_json(&format!("{API_URL}/api/trending"))?;
        let entries = response
            .trending
            .iter()
            .filter(|book| is_readable_english_epub(book))
            .map(|book| Self::item(book, false))
            .collect::<Result<Vec<_>>>()?;
        Ok(Paged::new(entries, false))
    }

    fn listing(
        &mut self,
        listing: &str,
        page: u32,
        _filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.popular(page),
            _ => Err(Error::new(format!("unknown BookRacy listing {listing:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().is_empty() {
            return self.popular(page);
        }
        let books = self.search_books(query, page, filters)?;
        let has_next = books.len() == PAGE_SIZE;
        Ok(Paged::new(
            books
                .iter()
                .map(|book| Self::item(book, false))
                .collect::<Result<Vec<_>>>()?,
            has_next,
        ))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let book = self.book_from_item(&item)?;
        Self::item(&book, true)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let book = self.book_from_item(&item)?;
        Ok(vec![Self::chapter(&book)?])
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let rendered = render_epub(&self.download_epub(url)?)?;
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

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter(
                "content",
                "Type",
                &[
                    ("Any book", "all"),
                    ("Fiction", "book_fiction"),
                    ("Non-fiction", "book_nonfiction"),
                    ("Other", "book_unknown"),
                ],
            ),
            select_filter(
                "sort",
                "Sort By",
                &[
                    ("Most relevant", "relevance"),
                    ("Newest", "newest"),
                    ("Oldest", "oldest"),
                    ("Largest", "largest"),
                    ("Smallest", "smallest"),
                ],
            ),
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        let md5 = if url.host_str() == Some("api.bookracy.com")
            && url.path().starts_with("/download/")
            && url.path().to_ascii_lowercase().ends_with(".epub")
        {
            url.path_segments()
                .and_then(|mut parts| parts.nth(1))
                .map(str::to_owned)
        } else if url.host_str() == Some("bookracy.com") {
            url.query_pairs()
                .find_map(|(key, value)| (key == "q").then(|| value.into_owned()))
                .filter(|value| is_valid_md5(value))
        } else {
            None
        };
        let Some(md5) = md5.filter(|value| is_valid_md5(value)) else {
            return Ok(None);
        };
        let md5 = md5.to_ascii_lowercase();
        let mut item = CatalogItem::new(md5.clone(), "");
        item.url = Some(book_url(&md5)?);
        item.language = Some("en".into());
        item.content_rating = Some("adult".into());
        item.extra.insert("bookracyMd5".into(), json!(md5));
        let novel_chapter = (url.host_str() == Some("api.bookracy.com")).then(|| NovelChapter {
            key: candidate.into(),
            title: Some("Full book".into()),
            chapter_number: Some(1.0),
            url: Some(candidate.into()),
            language: Some("en".into()),
            ..NovelChapter::default()
        });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter,
            ..UrlResolveResult::default()
        }))
    }
}

fn filter_value<'a>(filters: &'a Value, id: &str, default: &'a str) -> &'a str {
    filters
        .get(id)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(name, value)| OptionItem {
                label: (*name).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index: 0,
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn is_valid_md5(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn language_code(value: &str) -> Option<&str> {
    value
        .rsplit_once('[')
        .and_then(|(_, value)| value.strip_suffix(']'))
        .map(str::trim)
}

fn is_readable_english_epub(book: &Book) -> bool {
    book.book_filetype.eq_ignore_ascii_case("epub")
        && language_code(&book.book_lang)
            .map(|language| language.eq_ignore_ascii_case("en"))
            .unwrap_or(false)
}

fn metadata_tags(book: &Book) -> Vec<String> {
    [
        nonempty(&book.book_filetype).map(|value| value.to_ascii_uppercase()),
        nonempty(&book.year),
        nonempty(&book.series),
        nonempty(&book.publisher),
        nonempty(&book.book_size),
        nonempty(&book.isbn).map(|value| format!("ISBN {value}")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn book_url(md5: &str) -> Result<String> {
    let mut url = Url::parse(BASE_URL).map_err(|error| Error::new(error.to_string()))?;
    url.query_pairs_mut().append_pair("q", md5);
    Ok(url.to_string())
}

fn item_md5(item: &CatalogItem) -> Result<String> {
    if let Some(md5) = item
        .extra
        .get("bookracyMd5")
        .and_then(Value::as_str)
        .filter(|value| is_valid_md5(value))
    {
        return Ok(md5.to_ascii_lowercase());
    }
    if is_valid_md5(&item.key) {
        return Ok(item.key.to_ascii_lowercase());
    }
    let candidate = item.url.as_deref().unwrap_or(&item.key);
    let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
    url.query_pairs()
        .find_map(|(key, value)| (key == "q" && is_valid_md5(&value)).then(|| value.into_owned()))
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| Error::new("BookRacy item has no valid MD5"))
}

fn validate_download_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| Error::new(error.to_string()))?;
    require(
        (url.scheme() == "https"
            && url.host_str() == Some("api.bookracy.com")
            && url.path().starts_with("/download/")
            && url.path().to_ascii_lowercase().ends_with(".epub"))
        .then_some(()),
        "invalid BookRacy EPUB download URL",
    )
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
    let rootfile = normalize_archive_path(&capture(
        &container,
        r#"(?is)<rootfile\b[^>]*full-path=[\"']([^\"']+)[\"']"#,
    )?)?;
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
        let (Some(id), Some(href)) = (attribute(attrs, "id"), attribute(attrs, "href")) else {
            continue;
        };
        manifest.insert(
            id,
            (
                resolve_archive_path(base, &href)?,
                attribute(attrs, "media-type").unwrap_or_default(),
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
        if !html::text(html::document(&chapter).root_element())
            .trim()
            .is_empty()
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
    value: &str,
    base: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    manifest: &BTreeMap<String, (String, String)>,
) -> Result<String> {
    let pattern = Regex::new(r#"(?i)(src|href|xlink:href)=([\"'])([^\"'#][^\"']*)[\"']"#)
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(pattern
        .replace_all(value, |captures: &Captures<'_>| {
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
    let events = Regex::new(r#"(?i)\s+on[a-z]+\s*=\s*(?:\"[^\"]*\"|'[^']*')"#)
        .map_err(|error| Error::new(error.to_string()))?;
    rendered = events.replace_all(&rendered, "").into_owned();
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
    let mut clean = PathBuf::new();
    for part in Path::new(value.trim_start_matches('/')).components() {
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

fn capture(value: &str, pattern: &str) -> Result<String> {
    Regex::new(pattern)
        .map_err(|error| Error::new(error.to_string()))?
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| Error::new("required EPUB metadata is missing"))
}

fn attribute(value: &str, name: &str) -> Option<String> {
    Regex::new(&format!(
        r#"(?i)\b{}\s*=\s*[\"']([^\"']*)[\"']"#,
        regex::escape(name)
    ))
    .ok()?
    .captures(value)
    .and_then(|captures| captures.get(1))
    .map(|value| value.as_str().to_owned())
}

fn extract_body(value: &str) -> Option<String> {
    Regex::new(r"(?is)<body\b[^>]*>(.*)</body\s*>")
        .ok()?
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
}

fn should_skip_document(path: &str, value: &str, position: usize) -> bool {
    let lower_path = path.to_ascii_lowercase();
    let lower = format!("{lower_path} {}", value.to_ascii_lowercase());
    (position == 0 && (lower.contains("cover") || lower.contains("titlepage")))
        || ["toc", "copyright", "colophon", "about-the-publisher"]
            .iter()
            .any(|needle| lower_path.contains(needle))
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

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, BookRacySource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn fixture_book() -> Book {
        Book {
            title: "A Test Book".into(),
            author: "Test Author".into(),
            md5: "014551dfa41f52ef232bfb04a8944fa1".into(),
            link: "https://api.bookracy.com/download/014551dfa41f52ef232bfb04a8944fa1/A%20Test%20Book.epub?author=Test%20Author".into(),
            book_image: "https://api.bookracy.com/cover/014551dfa41f52ef232bfb04a8944fa1/thumbnail.jpg".into(),
            book_filetype: "epub".into(),
            book_lang: "English [en]".into(),
            book_size: "1.2MB".into(),
            description: "A test description.".into(),
            year: "2026".into(),
            ..Book::default()
        }
    }

    #[test]
    fn maps_bookracy_epub_metadata() {
        let item = BookRacySource::item(&fixture_book(), true).unwrap();
        assert_eq!(item.key, "014551dfa41f52ef232bfb04a8944fa1");
        assert_eq!(item.title, "A Test Book");
        assert_eq!(item.authors, ["Test Author"]);
        assert_eq!(item.language.as_deref(), Some("en"));
        assert!(item.cover.unwrap().url.contains("/cover/014551"));
        assert!(item.tags.contains(&"EPUB".to_owned()));
        assert!(item.initialized);
    }

    #[test]
    fn rejects_unsupported_catalog_formats() {
        let mut book = fixture_book();
        book.book_filetype = "pdf".into();
        assert!(!is_readable_english_epub(&book));
        assert!(BookRacySource::item(&book, false).is_err());
    }

    #[test]
    fn resolves_stable_item_and_download_urls() {
        let mut source = BookRacySource::default();
        let item = source
            .handle_url("https://bookracy.com/?q=014551dfa41f52ef232bfb04a8944fa1")
            .unwrap()
            .unwrap();
        assert_eq!(item.item.unwrap().key, "014551dfa41f52ef232bfb04a8944fa1");
        let chapter = source
            .handle_url(&fixture_book().link)
            .unwrap()
            .unwrap()
            .novel_chapter
            .unwrap();
        assert_eq!(chapter.chapter_number, Some(1.0));
    }

    #[test]
    fn blocks_arbitrary_download_urls() {
        assert!(validate_download_url("https://example.com/book.epub").is_err());
        assert!(validate_download_url("https://api.bookracy.com/cover/a/book.epub").is_err());
        assert!(validate_download_url(&fixture_book().link).is_ok());
    }

    #[test]
    fn renders_epub_spine_and_inline_images() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = SimpleFileOptions::default();
            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();
            zip.start_file("OEBPS/content.opf", options).unwrap();
            zip.write_all(br#"<package><manifest><item id="c1" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="pic" href="pic.png" media-type="image/png"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
            zip.start_file("OEBPS/chapter.xhtml", options).unwrap();
            zip.write_all(br#"<html><body><h1>Chapter</h1><p>Readable text.</p><img src="pic.png"/><script>bad()</script></body></html>"#).unwrap();
            zip.start_file("OEBPS/pic.png", options).unwrap();
            zip.write_all(b"PNG").unwrap();
            zip.finish().unwrap();
        }
        let rendered = render_epub(cursor.get_ref()).unwrap();
        assert!(rendered.contains("Readable text."));
        assert!(rendered.contains("data:image/png;base64,"));
        assert!(!rendered.contains("bad()"));
    }

    #[test]
    fn prevents_archive_path_traversal() {
        assert!(normalize_archive_path("../secret").is_err());
        assert!(resolve_archive_path(Path::new("OEBPS"), "../../secret").is_err());
    }
}
