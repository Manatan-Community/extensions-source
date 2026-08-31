use std::{collections::BTreeMap, io::Read};

use flate2::read::DeflateDecoder;
use manatan_sdk::{
    client::{Client, BROWSER_USER_AGENT},
    CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage, MangaSource,
    OptionItem, PageContent, Paged, ProcessedImage, Result, UrlResolveResult,
};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const CATALOG_URL: &str = "https://mokuro.moe/catalog";
const CATALOG_API_LIBRARY_URL: &str = "https://mokuro.moe/catalog/api/library";
const COVER_API_URL: &str = "https://mokuro.moe/catalog/api/cover";
const READER_FILES_URL: &str = "https://mokuro.moe/mokuro-reader";
const WEB_READER_URL: &str = "https://reader.mokuro.app/#/upload";
const LANGUAGE: &str = "ja";
const CONTENT_RATING: &str = "adult";
const PAGE_SIZE: usize = 40;
const REQUEST_LIMIT_MS: u32 = 100;
const ZIP_TAIL_SIZE: u64 = 65_557;
const MAX_CENTRAL_DIRECTORY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PAGE_BYTES: u64 = 32 * 1024 * 1024;

pub struct MokuroSource {
    client: Client,
}

impl Default for MokuroSource {
    fn default() -> Self {
        Self {
            client: Client::browser()
                .header("Accept", "application/json, */*;q=0.8")
                .header("Referer", CATALOG_URL),
        }
    }
}

impl MokuroSource {
    fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        self.client
            .get(url)
            .rate_limit("mokuro", REQUEST_LIMIT_MS)
            .send()?
            .error_for_status()?
            .json()
    }

    fn library(&self) -> Result<LibraryResponse> {
        self.get_json(&format!("{READER_FILES_URL}/catalog.json"))
    }

    fn series(&self, name: &str) -> Result<SeriesResponse> {
        self.get_json(&reader_series_url(name)?)
    }

    fn catalog_covers(&self) -> BTreeMap<String, String> {
        self.get_json::<CatalogApiLibraryResponse>(CATALOG_API_LIBRARY_URL)
            .map(|library| {
                library
                    .series
                    .into_iter()
                    .filter_map(|series| series.cover.map(|cover| (series.name, cover)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn list(
        &self,
        page: u32,
        query: &str,
        filters: &Value,
        forced_sort: Option<&str>,
    ) -> Result<Paged<CatalogItem>> {
        let mut entries = self.library()?.series;
        let query = query.trim().to_lowercase();
        if !query.is_empty() {
            entries.retain(|series| series.matches(&query));
        }
        sort_series(
            &mut entries,
            forced_sort
                .or_else(|| selected(filters, "sort"))
                .unwrap_or("title"),
        );
        paginate(entries, page, &self.catalog_covers())
    }
}

impl MangaSource for MokuroSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.list(page, "", &Value::Null, Some("title"))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.list(page, "", &Value::Null, Some("newest"))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().starts_with("https://") {
            if let Some(item) = self
                .handle_url(query.trim())?
                .and_then(|result| result.item)
            {
                return Ok(Paged::new(vec![item], false));
            }
        }
        self.list(page, query, filters, None)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let key = item_key(&item)?;
        let summary = self
            .library()?
            .series
            .into_iter()
            .find(|series| series.series_title == key)
            .ok_or_else(|| Error::new(format!("Mokuro series not found: {key}")))?;
        let series = self.series(&key)?;
        let mut details = summary.to_item(None)?;
        series.enrich_item(&mut details);
        details.initialized = true;
        Ok(details)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let series_name = item_key(&item)?;
        let series = self.series(&series_name)?;
        if series.volumes.is_empty() {
            return Err(Error::new("Mokuro series has no readable volumes"));
        }
        let volume_count = series.volumes.len();
        Ok(series
            .volumes
            .into_iter()
            .rev()
            .enumerate()
            .map(|(index, volume)| volume.to_chapter(&series_name, volume_count - index))
            .collect())
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let series_name = item_key(&item)?;
        let volume_name = chapter_volume_name(&chapter)?;
        let mokuro_url = reader_file_url(&series_name, &volume_name, "mokuro")?;
        let cbz_url = reader_file_url(&series_name, &volume_name, "cbz")?;
        let mokuro: MokuroVolume = self.get_json(&mokuro_url)?;
        let entries = fetch_zip_entries(&self.client, &cbz_url)?;
        map_mokuro_pages(mokuro, &entries, &cbz_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "sort".to_owned(),
            name: "Sort by".to_owned(),
            options: vec![
                option("Title", "title"),
                option("Recently updated", "newest"),
            ],
            default_index: 0,
        }])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(catalog_item_url(&item_key(item)?)?))
    }

    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let cbz_url = reader_file_url(&item_key(item)?, &chapter_volume_name(chapter)?, "cbz")?;
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("cbz", &cbz_url)
            .finish();
        Ok(Some(format!("{WEB_READER_URL}?{query}")))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(url_error)?;
        if url.host_str() != Some("mokuro.moe")
            || !url.path().trim_end_matches('/').ends_with("/catalog")
        {
            return Ok(None);
        }
        let Some(fragment) = url.fragment().filter(|fragment| !fragment.is_empty()) else {
            return Ok(None);
        };
        let key = decode_component(fragment);
        if key.is_empty() {
            return Ok(None);
        }
        let series = self
            .library()?
            .series
            .into_iter()
            .find(|series| series.series_title == key);
        Ok(series
            .map(|series| series.to_item(None))
            .transpose()?
            .map(|item| UrlResolveResult {
                item: Some(item),
                ..UrlResolveResult::default()
            }))
    }

    fn process_page_image(
        &mut self,
        _item: &CatalogItem,
        _chapter: &MangaChapter,
        page: &MangaPage,
        image: &[u8],
        _mime_type: Option<&str>,
    ) -> Result<Option<ProcessedImage>> {
        let metadata = ZipPageMetadata::from_page(page)?;
        let bytes = decode_local_zip_entry(image, &metadata)?;
        Ok(Some(ProcessedImage {
            bytes,
            mime_type: Some(image_mime_type(&metadata.name).to_owned()),
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("mokuro", MokuroSource::default())
);

#[derive(Clone, Debug, Deserialize)]
struct LibraryResponse {
    series: Vec<SeriesSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct CatalogApiLibraryResponse {
    series: Vec<CatalogApiSeriesSummary>,
}

#[derive(Clone, Debug, Deserialize)]
struct CatalogApiSeriesSummary {
    name: String,
    cover: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct SeriesSummary {
    series_title: String,
    #[serde(default)]
    titles: Titles,
    #[serde(default)]
    synonyms: Vec<String>,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    external_ids: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Titles {
    native: Option<String>,
    romaji: Option<String>,
    english: Option<String>,
}

impl SeriesSummary {
    fn title(&self) -> &str {
        self.titles
            .native
            .as_deref()
            .or(self.titles.romaji.as_deref())
            .or(self.titles.english.as_deref())
            .unwrap_or(&self.series_title)
    }

    fn matches(&self, query: &str) -> bool {
        [
            Some(self.series_title.as_str()),
            self.titles.native.as_deref(),
            self.titles.romaji.as_deref(),
            self.titles.english.as_deref(),
        ]
        .into_iter()
        .flatten()
        .chain(self.synonyms.iter().map(String::as_str))
        .any(|value| value.to_lowercase().contains(query))
    }

    fn to_item(&self, cover_path: Option<&str>) -> Result<CatalogItem> {
        let mut alternate_titles = [
            self.titles.native.as_deref(),
            self.titles.romaji.as_deref(),
            self.titles.english.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|title| *title != self.title())
        .map(ToOwned::to_owned)
        .chain(self.synonyms.iter().cloned())
        .collect::<Vec<_>>();
        alternate_titles.sort();
        alternate_titles.dedup();
        let mut description = vec![format!("WebDAV folder: {}", self.series_title)];
        if !alternate_titles.is_empty() {
            description.push(format!(
                "Alternate titles: {}",
                alternate_titles.join(" / ")
            ));
        }
        if !self.updated_at.is_empty() {
            description.push(format!("Updated: {}", self.updated_at));
        }
        let cover = cover_path
            .map(cover_url)
            .transpose()?
            .map(|url| ImageRequest::get(url).header("Referer", CATALOG_URL));
        let mut extra = BTreeMap::new();
        extra.insert("folderName".to_owned(), json!(self.series_title));
        extra.insert("externalIds".to_owned(), json!(self.external_ids));
        Ok(CatalogItem {
            key: self.series_title.clone(),
            title: self.title().to_owned(),
            url: Some(catalog_item_url(&self.series_title)?),
            cover,
            description: Some(description.join("\n")),
            content_rating: Some(CONTENT_RATING.to_owned()),
            language: Some(LANGUAGE.to_owned()),
            viewer: Some(json!("rtl")),
            extra,
            ..CatalogItem::default()
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct SeriesResponse {
    series_title: String,
    #[serde(default)]
    updated_at: String,
    volumes: Vec<VolumeSummary>,
}

impl SeriesResponse {
    fn enrich_item(&self, item: &mut CatalogItem) {
        let volume_count = self.volumes.len();
        let page_count = self
            .volumes
            .iter()
            .map(|volume| volume.page_count as u64)
            .sum::<u64>();
        let matched_page_count = self
            .volumes
            .iter()
            .map(|volume| volume.matched_page_count as u64)
            .sum::<u64>();
        let character_count = self
            .volumes
            .iter()
            .map(|volume| volume.character_count)
            .sum::<u64>();
        let missing_pages = page_count.saturating_sub(matched_page_count);
        let mut description = item.description.take().unwrap_or_default();
        if !description.is_empty() {
            description.push('\n');
        }
        description.push_str(&format!(
            "{} volumes · {} pages · {} OCR characters",
            volume_count, page_count, character_count
        ));
        if missing_pages > 0 {
            description.push_str(&format!(
                "\nWarning: {missing_pages} referenced pages are missing."
            ));
        }
        item.description = Some(description);
        if let Some(first_volume) = self.volumes.first() {
            item.cover = reader_file_url(&self.series_title, &first_volume.volume_title, "webp")
                .ok()
                .map(ImageRequest::get);
        }
        item.extra
            .insert("seriesTitle".to_owned(), json!(self.series_title));
        item.extra
            .insert("updatedAt".to_owned(), json!(self.updated_at));
        item.extra
            .insert("volumeCount".to_owned(), json!(volume_count));
        item.extra.insert("pageCount".to_owned(), json!(page_count));
        item.extra
            .insert("missingPages".to_owned(), json!(missing_pages));
    }
}

#[derive(Clone, Debug, Deserialize)]
struct VolumeSummary {
    volume_title: String,
    #[serde(default)]
    page_count: u32,
    #[serde(default)]
    matched_page_count: u32,
    #[serde(default)]
    character_count: u64,
    #[serde(default)]
    mokuro_modified: i64,
}

impl VolumeSummary {
    fn to_chapter(&self, series_name: &str, source_order: usize) -> MangaChapter {
        let missing_pages = self.page_count.saturating_sub(self.matched_page_count);
        let summary = (missing_pages > 0).then(|| {
            format!(
                "{missing_pages} referenced pages are missing. {} OCR characters.",
                self.character_count
            )
        });
        MangaChapter {
            key: self.volume_title.clone(),
            title: Some(self.volume_title.clone()),
            chapter_number: number_in_text(&self.volume_title),
            volume_number: number_in_text(&self.volume_title),
            date_uploaded: (self.mokuro_modified > 0).then_some(self.mokuro_modified),
            language: Some(LANGUAGE.to_owned()),
            thumbnail: reader_file_url(series_name, &self.volume_title, "webp")
                .ok()
                .map(ImageRequest::get),
            url: reader_file_url(series_name, &self.volume_title, "cbz").ok(),
            source_order: Some(source_order as i32),
            page_count: Some(self.matched_page_count),
            summary,
            ..MangaChapter::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct MokuroVolume {
    pages: Vec<MokuroPage>,
}

#[derive(Clone, Debug, Deserialize)]
struct MokuroPage {
    img_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ZipEntry {
    name: String,
    local_header_offset: u64,
    compressed_size: u64,
    uncompressed_size: u64,
    method: u16,
    central_name_length: u16,
    central_extra_length: u16,
}

#[derive(Clone, Debug)]
struct ZipPageMetadata {
    name: String,
    compressed_size: u64,
    uncompressed_size: u64,
    method: u16,
}

impl ZipPageMetadata {
    fn from_page(page: &MangaPage) -> Result<Self> {
        Ok(Self {
            name: page_extra_string(page, "zipEntryName")?.to_owned(),
            compressed_size: page_extra_u64(page, "zipCompressedSize")?,
            uncompressed_size: page_extra_u64(page, "zipUncompressedSize")?,
            method: u16::try_from(page_extra_u64(page, "zipCompressionMethod")?)
                .map_err(|_| Error::new("invalid ZIP compression method"))?,
        })
    }
}

fn fetch_zip_entries(client: &Client, url: &str) -> Result<Vec<ZipEntry>> {
    // A one-byte range probe gives us the archive size without relying on a
    // HEAD response whose Content-Length describes the entire (often >64 MiB)
    // CBZ and can therefore trip a host response-size guard.
    let probe = client
        .get(url)
        .header("Range", "bytes=0-0")
        .max_body_bytes(1)
        .rate_limit("mokuro", REQUEST_LIMIT_MS)
        .send()?
        .error_for_status()?;
    if probe.status() != 206 {
        return Err(Error::new(
            "Mokuro CBZ server ignored the archive-size range request",
        ));
    }
    let total_size = probe
        .header("content-range")
        .and_then(parse_content_range_total)
        .ok_or_else(|| Error::new("Mokuro CBZ response has no valid Content-Range"))?;
    let tail_start = total_size.saturating_sub(ZIP_TAIL_SIZE);
    let tail = client
        .get(url)
        .header("Range", format!("bytes={tail_start}-{}", total_size - 1))
        .max_body_bytes(ZIP_TAIL_SIZE)
        .rate_limit("mokuro", REQUEST_LIMIT_MS)
        .send()?
        .error_for_status()?;
    if tail.status() != 206 {
        return Err(Error::new(
            "Mokuro CBZ server ignored the ZIP tail range request",
        ));
    }
    let (directory_offset, directory_size) = parse_eocd(tail.bytes())?;
    if directory_size > MAX_CENTRAL_DIRECTORY_BYTES {
        return Err(Error::new("Mokuro CBZ central directory is too large"));
    }
    let directory_end = directory_offset
        .checked_add(directory_size)
        .ok_or_else(|| Error::new("Mokuro CBZ central directory overflow"))?;
    if directory_end > total_size {
        return Err(Error::new(
            "Mokuro CBZ central directory is outside the archive",
        ));
    }
    let directory = if directory_offset >= tail_start && directory_end <= total_size {
        let start = usize::try_from(directory_offset - tail_start)
            .map_err(|_| Error::new("Mokuro CBZ directory offset is invalid"))?;
        let end = start
            .checked_add(directory_size as usize)
            .ok_or_else(|| Error::new("Mokuro CBZ directory size overflow"))?;
        tail.bytes()
            .get(start..end)
            .ok_or_else(|| Error::new("Mokuro CBZ tail does not contain its directory"))?
            .to_vec()
    } else {
        client
            .get(url)
            .header(
                "Range",
                format!("bytes={directory_offset}-{}", directory_end - 1),
            )
            .max_body_bytes(directory_size)
            .rate_limit("mokuro", REQUEST_LIMIT_MS)
            .send()?
            .error_for_status()?
            .into_bytes()
    };
    parse_central_directory(&directory)
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    let (unit_and_range, total) = value.trim().rsplit_once('/')?;
    let (unit, range) = unit_and_range.split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") || !range.contains('-') {
        return None;
    }
    total.parse().ok().filter(|total| *total > 0)
}

fn parse_eocd(tail: &[u8]) -> Result<(u64, u64)> {
    let position = tail
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .ok_or_else(|| Error::new("Mokuro CBZ has no ZIP end-of-central-directory record"))?;
    let record = tail
        .get(position..)
        .ok_or_else(|| Error::new("invalid ZIP end record"))?;
    if record.len() < 22 {
        return Err(Error::new("truncated ZIP end record"));
    }
    let disk = read_u16(record, 4)?;
    let directory_disk = read_u16(record, 6)?;
    let entries_on_disk = read_u16(record, 8)?;
    let entries = read_u16(record, 10)?;
    let directory_size = read_u32(record, 12)?;
    let directory_offset = read_u32(record, 16)?;
    if disk != 0 || directory_disk != 0 || entries_on_disk != entries {
        return Err(Error::new("multi-disk Mokuro CBZ archives are unsupported"));
    }
    if entries == u16::MAX || directory_size == u32::MAX || directory_offset == u32::MAX {
        return Err(Error::new("ZIP64 Mokuro CBZ archives are unsupported"));
    }
    Ok((directory_offset as u64, directory_size as u64))
}

fn parse_central_directory(bytes: &[u8]) -> Result<Vec<ZipEntry>> {
    let mut entries = Vec::new();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let record = bytes
            .get(offset..)
            .ok_or_else(|| Error::new("invalid ZIP central directory offset"))?;
        if record.len() < 46 || record.get(..4) != Some(b"PK\x01\x02") {
            return Err(Error::new("invalid Mokuro CBZ central directory entry"));
        }
        let method = read_u16(record, 10)?;
        let compressed_size = read_u32(record, 20)?;
        let uncompressed_size = read_u32(record, 24)?;
        let name_length = read_u16(record, 28)?;
        let extra_length = read_u16(record, 30)?;
        let comment_length = read_u16(record, 32)?;
        let disk = read_u16(record, 34)?;
        let local_header_offset = read_u32(record, 42)?;
        if disk != 0
            || compressed_size == u32::MAX
            || uncompressed_size == u32::MAX
            || local_header_offset == u32::MAX
        {
            return Err(Error::new(
                "ZIP64 or multi-disk Mokuro CBZ entry is unsupported",
            ));
        }
        let record_length = 46_usize
            .checked_add(name_length as usize)
            .and_then(|value| value.checked_add(extra_length as usize))
            .and_then(|value| value.checked_add(comment_length as usize))
            .ok_or_else(|| Error::new("Mokuro CBZ directory entry size overflow"))?;
        let name_end = 46 + name_length as usize;
        let name = std::str::from_utf8(
            record
                .get(46..name_end)
                .ok_or_else(|| Error::new("truncated Mokuro CBZ entry name"))?,
        )
        .map_err(|error| Error::new(format!("invalid Mokuro CBZ entry name: {error}")))?
        .to_owned();
        entries.push(ZipEntry {
            name,
            local_header_offset: local_header_offset as u64,
            compressed_size: compressed_size as u64,
            uncompressed_size: uncompressed_size as u64,
            method,
            central_name_length: name_length,
            central_extra_length: extra_length,
        });
        if entries.len() > 10_000 {
            return Err(Error::new("Mokuro CBZ contains too many entries"));
        }
        offset = offset
            .checked_add(record_length)
            .ok_or_else(|| Error::new("Mokuro CBZ directory offset overflow"))?;
    }
    if entries.is_empty() {
        return Err(Error::new("Mokuro CBZ central directory is empty"));
    }
    Ok(entries)
}

fn map_mokuro_pages(
    mokuro: MokuroVolume,
    entries: &[ZipEntry],
    cbz_url: &str,
) -> Result<Vec<MangaPage>> {
    let by_name = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut pages = Vec::new();
    for mokuro_page in mokuro.pages {
        let Some(entry) = by_name.get(mokuro_page.img_path.as_str()).copied() else {
            continue;
        };
        if !matches!(entry.method, 0 | 8) || entry.uncompressed_size > MAX_PAGE_BYTES {
            continue;
        }
        let local_record_size = 30_u64
            .checked_add(entry.central_name_length as u64)
            .and_then(|value| value.checked_add(entry.central_extra_length as u64))
            .and_then(|value| value.checked_add(entry.compressed_size))
            .ok_or_else(|| Error::new("Mokuro CBZ page range overflow"))?;
        let range_end = entry
            .local_header_offset
            .checked_add(local_record_size)
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| Error::new("Mokuro CBZ page range is invalid"))?;
        let mut context = BTreeMap::new();
        context.insert(
            "Range".to_owned(),
            format!("bytes={}-{}", entry.local_header_offset, range_end),
        );
        context.insert("Referer".to_owned(), CATALOG_URL.to_owned());
        context.insert("User-Agent".to_owned(), BROWSER_USER_AGENT.to_owned());
        let mut extra = BTreeMap::new();
        extra.insert("zipEntryName".to_owned(), json!(entry.name));
        extra.insert("zipCompressedSize".to_owned(), json!(entry.compressed_size));
        extra.insert(
            "zipUncompressedSize".to_owned(),
            json!(entry.uncompressed_size),
        );
        extra.insert("zipCompressionMethod".to_owned(), json!(entry.method));
        pages.push(MangaPage {
            content: PageContent::Url {
                url: cbz_url.to_owned(),
                context: Some(context),
            },
            description: Some(format!("Page {}", pages.len() + 1)),
            extra,
            ..MangaPage::default()
        });
    }
    if pages.is_empty() {
        return Err(Error::new("Mokuro volume has no readable CBZ images"));
    }
    Ok(pages)
}

fn decode_local_zip_entry(bytes: &[u8], metadata: &ZipPageMetadata) -> Result<Vec<u8>> {
    if bytes.len() < 30 || bytes.get(..4) != Some(b"PK\x03\x04") {
        return Err(Error::new("Mokuro page range has no ZIP local header"));
    }
    let method = read_u16(bytes, 8)?;
    if method != metadata.method {
        return Err(Error::new("Mokuro page ZIP compression method changed"));
    }
    let name_length = read_u16(bytes, 26)? as usize;
    let extra_length = read_u16(bytes, 28)? as usize;
    let data_start = 30_usize
        .checked_add(name_length)
        .and_then(|value| value.checked_add(extra_length))
        .ok_or_else(|| Error::new("Mokuro page ZIP header overflow"))?;
    let data_end = data_start
        .checked_add(metadata.compressed_size as usize)
        .ok_or_else(|| Error::new("Mokuro page compressed size overflow"))?;
    let compressed = bytes
        .get(data_start..data_end)
        .ok_or_else(|| Error::new("Mokuro page range is truncated"))?;
    let decoded = match metadata.method {
        0 => compressed.to_vec(),
        8 => {
            let mut output = Vec::with_capacity(metadata.uncompressed_size as usize);
            DeflateDecoder::new(compressed)
                .take(metadata.uncompressed_size.saturating_add(1))
                .read_to_end(&mut output)
                .map_err(|error| {
                    Error::new(format!("Mokuro page decompression failed: {error}"))
                })?;
            output
        }
        method => {
            return Err(Error::new(format!(
                "unsupported Mokuro CBZ compression method {method}"
            )))
        }
    };
    if decoded.len() as u64 != metadata.uncompressed_size {
        return Err(Error::new(format!(
            "Mokuro page size mismatch for {}: expected {}, got {}",
            metadata.name,
            metadata.uncompressed_size,
            decoded.len()
        )));
    }
    Ok(decoded)
}

fn paginate(
    entries: Vec<SeriesSummary>,
    page: u32,
    covers: &BTreeMap<String, String>,
) -> Result<Paged<CatalogItem>> {
    let page = page.max(1) as usize;
    let start = (page - 1).saturating_mul(PAGE_SIZE);
    if start >= entries.len() {
        return Ok(Paged::new(Vec::new(), false));
    }
    let end = start.saturating_add(PAGE_SIZE).min(entries.len());
    let has_next = end < entries.len();
    let entries = entries[start..end]
        .iter()
        .map(|series| series.to_item(covers.get(&series.series_title).map(String::as_str)))
        .collect::<Result<Vec<_>>>()?;
    Ok(Paged::new(entries, has_next))
}

fn sort_series(series: &mut [SeriesSummary], sort: &str) {
    match sort {
        "newest" => series.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| title_cmp(left, right))
        }),
        _ => series.sort_by(title_cmp),
    }
}

fn title_cmp(left: &SeriesSummary, right: &SeriesSummary) -> std::cmp::Ordering {
    left.title()
        .to_lowercase()
        .cmp(&right.title().to_lowercase())
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

fn selected<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn cover_url(path: &str) -> Result<String> {
    let mut url = Url::parse(COVER_API_URL).map_err(url_error)?;
    url.query_pairs_mut().append_pair("path", path);
    Ok(url.to_string())
}

fn reader_series_url(series_name: &str) -> Result<String> {
    let mut url = Url::parse(READER_FILES_URL).map_err(url_error)?;
    url.path_segments_mut()
        .map_err(|_| Error::new("Mokuro reader URL cannot accept path segments"))?
        .push(series_name)
        .push("series.json");
    Ok(url.to_string())
}

fn catalog_item_url(series_name: &str) -> Result<String> {
    let mut url = Url::parse(CATALOG_URL).map_err(url_error)?;
    url.set_fragment(Some(series_name));
    Ok(url.to_string())
}

fn reader_file_url(series_name: &str, volume_name: &str, extension: &str) -> Result<String> {
    let mut url = Url::parse(READER_FILES_URL).map_err(url_error)?;
    url.path_segments_mut()
        .map_err(|_| Error::new("Mokuro reader URL cannot accept path segments"))?
        .push(series_name)
        .push(&format!("{volume_name}.{extension}"));
    Ok(url.to_string())
}

fn item_key(item: &CatalogItem) -> Result<String> {
    if !item.key.trim().is_empty() {
        return Ok(item.key.clone());
    }
    let url = item
        .url
        .as_deref()
        .ok_or_else(|| Error::new("Mokuro item has no series key"))?;
    let parsed = Url::parse(url).map_err(url_error)?;
    parsed
        .fragment()
        .map(decode_component)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("Mokuro item URL has no series key"))
}

fn chapter_volume_name(chapter: &MangaChapter) -> Result<String> {
    if !chapter.key.trim().is_empty() {
        return Ok(chapter.key.clone());
    }
    Err(Error::new("Mokuro chapter has no volume name"))
}

fn decode_component(value: &str) -> String {
    url::form_urlencoded::parse(value.as_bytes())
        .next()
        .map(|(decoded, _)| decoded.into_owned())
        .unwrap_or_default()
}

fn number_in_text(value: &str) -> Option<f32> {
    let mut current = String::new();
    let mut last = None;
    for character in value.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() || (character == '.' && !current.is_empty()) {
            current.push(character);
        } else if !current.is_empty() {
            last = current.parse::<f32>().ok().or(last);
            current.clear();
        }
    }
    last
}

fn image_mime_type(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".avif") {
        "image/avif"
    } else {
        "image/jpeg"
    }
}

fn page_extra_string<'a>(page: &'a MangaPage, key: &str) -> Result<&'a str> {
    page.extra
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new(format!("Mokuro page is missing {key}")))
}

fn page_extra_u64(page: &MangaPage, key: &str) -> Result<u64> {
    page.extra
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::new(format!("Mokuro page is missing {key}")))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::new("truncated ZIP record"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::new("truncated ZIP record"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("invalid Mokuro URL: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    const LIBRARY: &str = include_str!("../fixtures/library.json");
    const SERIES: &str = include_str!("../fixtures/series.json");
    const VOLUME: &str = include_str!("../fixtures/volume.mokuro");

    #[test]
    fn parses_webdav_catalog_manifest_without_embedded_volumes() {
        let library: LibraryResponse = serde_json::from_str(LIBRARY).unwrap();
        assert_eq!(library.series.len(), 2);
        let yotsuba = &library.series[0];
        let item = yotsuba.to_item(None).unwrap();
        assert_eq!(item.key, "Yotsuba to!");
        assert_eq!(item.title, "よつばと！");
        assert!(item.description.unwrap().contains("WebDAV folder"));
        assert!(yotsuba.matches("yotsuba&!"));
        assert!(yotsuba.matches("yotsubato"));
    }

    #[test]
    fn parses_webdav_series_manifest_into_latest_first_chapters() {
        let series: SeriesResponse = serde_json::from_str(SERIES).unwrap();
        let chapters = series
            .volumes
            .iter()
            .rev()
            .enumerate()
            .map(|(index, volume)| volume.to_chapter("Yotsuba to!", series.volumes.len() - index))
            .collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "Yotsuba-to--02");
        assert_eq!(chapters[0].source_order, Some(2));
        assert_eq!(chapters[0].volume_number, Some(2.0));
        assert_eq!(chapters[0].page_count, Some(223));
        assert!(chapters[0].summary.as_deref().unwrap().contains("missing"));
        assert_eq!(chapters[1].key, "Yotsuba-to--01");
        assert_eq!(chapters[1].source_order, Some(1));
    }

    #[test]
    fn maps_mokuro_paths_and_skips_declared_missing_images() {
        let mokuro: MokuroVolume = serde_json::from_str(VOLUME).unwrap();
        let entries = vec![
            ZipEntry {
                name: "Yotsuba-to--01/001.jpg".to_owned(),
                local_header_offset: 0,
                compressed_size: 10,
                uncompressed_size: 20,
                method: 8,
                central_name_length: 26,
                central_extra_length: 0,
            },
            ZipEntry {
                name: "Yotsuba-to--01/002.webp".to_owned(),
                local_header_offset: 66,
                compressed_size: 12,
                uncompressed_size: 22,
                method: 8,
                central_name_length: 27,
                central_extra_length: 0,
            },
        ];
        let pages = map_mokuro_pages(mokuro, &entries, "https://mokuro.moe/test.cbz").unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].description.as_deref(), Some("Page 1"));
        let PageContent::Url { url, context } = &pages[0].content else {
            panic!("expected ranged URL")
        };
        assert_eq!(url, "https://mokuro.moe/test.cbz");
        assert_eq!(
            context.as_ref().unwrap().get("Range").unwrap(),
            "bytes=0-65"
        );
    }

    #[test]
    fn parses_and_decodes_deflated_zip_entries() {
        let image = b"\xff\xd8fixture jpeg bytes\xff\xd9";
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "volume/001.jpg",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        archive.write_all(image).unwrap();
        let archive = archive.finish().unwrap().into_inner();
        let tail_start = archive.len().saturating_sub(ZIP_TAIL_SIZE as usize);
        let (directory_offset, directory_size) = parse_eocd(&archive[tail_start..]).unwrap();
        let directory =
            &archive[directory_offset as usize..(directory_offset + directory_size) as usize];
        let entries = parse_central_directory(directory).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        let record_end = entry.local_header_offset as usize
            + 30
            + entry.central_name_length as usize
            + entry.central_extra_length as usize
            + entry.compressed_size as usize;
        let metadata = ZipPageMetadata {
            name: entry.name.clone(),
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            method: entry.method,
        };
        let decoded = decode_local_zip_entry(
            &archive[entry.local_header_offset as usize..record_end],
            &metadata,
        )
        .unwrap();
        assert_eq!(decoded, image);
    }

    #[test]
    fn resolves_catalog_hash_and_encodes_reader_paths() {
        let item_url = catalog_item_url("Yotsuba to!").unwrap();
        assert_eq!(item_url, "https://mokuro.moe/catalog#Yotsuba%20to!");
        let parsed = Url::parse(&item_url).unwrap();
        assert_eq!(decode_component(parsed.fragment().unwrap()), "Yotsuba to!");
        assert_eq!(
            reader_file_url("#Zombie Sagashitemasu", "Volume 1", "cbz").unwrap(),
            "https://mokuro.moe/mokuro-reader/%23Zombie%20Sagashitemasu/Volume%201.cbz"
        );
        assert_eq!(
            reader_series_url("Yotsuba to!").unwrap(),
            "https://mokuro.moe/mokuro-reader/Yotsuba%20to!/series.json"
        );
        let mut source = MokuroSource::default();
        let item = CatalogItem::new("#Zombie Sagashitemasu", "Zombie");
        let chapter = MangaChapter {
            key: "Volume 1".to_owned(),
            ..MangaChapter::default()
        };
        assert_eq!(
            source.chapter_url(&item, &chapter).unwrap().unwrap(),
            "https://reader.mokuro.app/#/upload?cbz=https%3A%2F%2Fmokuro.moe%2Fmokuro-reader%2F%2523Zombie%2520Sagashitemasu%2FVolume%25201.cbz"
        );
    }

    #[test]
    fn parses_last_number_from_volume_names() {
        assert_eq!(number_in_text("Series v2.5 revised 03"), Some(3.0));
        assert_eq!(number_in_text("Yotsuba-to--15"), Some(15.0));
        assert_eq!(number_in_text("Special"), None);
    }

    #[test]
    fn parses_total_archive_size_from_content_range() {
        assert_eq!(
            parse_content_range_total("bytes 0-0/70371434"),
            Some(70_371_434)
        );
        assert_eq!(parse_content_range_total("bytes */70371434"), None);
        assert_eq!(parse_content_range_total("70371434"), None);
    }
}
