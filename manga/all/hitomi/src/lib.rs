// Hitomi catalog protocol behavior is based on the Apache-2.0 implementation in
// yuzono/cursed-manga-extensions. This is a fresh Rust implementation for Manatan.

use std::collections::BTreeSet;

use chrono::DateTime;
use manatan_sdk::{
    client::Client, CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage,
    MangaSource, OptionItem, PageContent, Paged, Result, UrlResolveResult,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

const BASE_URL: &str = "https://hitomi.la";
const CDN_DOMAIN: &str = "gold-usergeneratedcontent.net";
const LTN_URL: &str = "https://ltn.gold-usergeneratedcontent.net";
const PAGE_SIZE: usize = 25;
const NODE_SIZE: u64 = 464;
const MAX_NOZOMI_BYTES: u64 = 16_000_000;
const MAX_SEARCH_DATA_BYTES: u64 = 100_000_000;

pub struct HitomiSource {
    client: Client,
    source_language: &'static str,
    nozomi_language: &'static str,
    index_version: Option<String>,
    image_config: Option<ImageConfig>,
}

impl Default for HitomiSource {
    fn default() -> Self {
        Self::new("all", "all")
    }
}

impl HitomiSource {
    fn new(source_language: &'static str, nozomi_language: &'static str) -> Self {
        Self {
            client: Client::browser()
                .header("Referer", format!("{BASE_URL}/"))
                .header("Origin", BASE_URL),
            source_language,
            nozomi_language,
            index_version: None,
            image_config: None,
        }
    }

    fn ranged_bytes(&self, url: &str, start: Option<u64>, length: u64) -> Result<Vec<u8>> {
        let mut request = self.client.get(url).max_body_bytes(length.max(1));
        if let Some(start) = start {
            let end = start
                .checked_add(length.saturating_sub(1))
                .ok_or_else(|| Error::new("Hitomi byte range overflow"))?;
            request = request.header("Range", format!("bytes={start}-{end}"));
        }
        Ok(request.send()?.error_for_status()?.into_bytes())
    }

    fn text(&self, url: &str, max_body_bytes: u64) -> Result<String> {
        self.client
            .get(url)
            .max_body_bytes(max_body_bytes)
            .send()?
            .error_for_status()?
            .text()
            .map(ToOwned::to_owned)
    }

    fn nozomi_url(area: Option<&str>, tag: &str, language: &str) -> String {
        match area {
            Some(area) => format!("{LTN_URL}/{area}/{tag}-{language}.nozomi"),
            None => format!("{LTN_URL}/{tag}-{language}.nozomi"),
        }
    }

    fn nozomi_ids(
        &self,
        area: Option<&str>,
        tag: &str,
        language: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u32>> {
        let url = Self::nozomi_url(area, tag, language);
        let bytes = match range {
            Some((start, length)) => self.ranged_bytes(&url, Some(start), length)?,
            None => self.ranged_bytes(&url, None, MAX_NOZOMI_BYTES)?,
        };
        decode_u32_list(&bytes)
    }

    fn listing_ids(&self, area: Option<&str>, tag: &str, page: u32) -> Result<Vec<u32>> {
        let start = u64::from(page.max(1) - 1) * PAGE_SIZE as u64 * 4;
        self.nozomi_ids(
            area,
            tag,
            self.nozomi_language,
            Some((start, (PAGE_SIZE * 4) as u64)),
        )
    }

    fn gallery(&self, id: u32) -> Result<Gallery> {
        let script = self.text(&format!("{LTN_URL}/galleries/{id}.js"), 4_000_000)?;
        parse_gallery_script(&script)
    }

    fn galleries(&mut self, ids: &[u32]) -> Result<Vec<CatalogItem>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let requests = ids
            .iter()
            .map(|id| {
                self.client
                    .get(format!("{LTN_URL}/galleries/{id}.js"))
                    .max_body_bytes(4_000_000)
            })
            .collect();
        let mut entries = Vec::new();
        let mut first_error = None;
        for response in Client::send_many(requests, 8) {
            match response
                .and_then(|response| response.error_for_status())
                .and_then(|response| parse_gallery_script(response.text()?))
                .and_then(|gallery| self.gallery_to_item(gallery))
            {
                Ok(entry) => entries.push(entry),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if entries.is_empty() {
            return Err(first_error.unwrap_or_else(|| Error::new("Hitomi returned no galleries")));
        }
        Ok(entries)
    }

    fn listing_page(
        &mut self,
        area: Option<&str>,
        tag: &str,
        page: u32,
    ) -> Result<Paged<CatalogItem>> {
        let ids = self.listing_ids(area, tag, page)?;
        let has_next_page = ids.len() == PAGE_SIZE;
        Ok(Paged::new(self.galleries(&ids)?, has_next_page))
    }

    fn gallery_to_item(&mut self, gallery: Gallery) -> Result<CatalogItem> {
        let id = gallery_id_from_path(&gallery.galleryurl)?;
        let cover = gallery
            .files
            .first()
            .map(|file| self.image_request(file, true, id))
            .transpose()?;
        let authors = gallery
            .groups
            .as_ref()
            .filter(|values| !values.is_empty())
            .map(|values| {
                values
                    .iter()
                    .map(|value| title_case(&value.group))
                    .collect()
            })
            .unwrap_or_else(|| {
                gallery
                    .artists
                    .iter()
                    .flatten()
                    .map(|value| title_case(&value.artist))
                    .collect()
            });
        let artists = gallery
            .artists
            .iter()
            .flatten()
            .map(|value| title_case(&value.artist))
            .collect();
        let tags = gallery.tags.iter().flatten().map(Tag::formatted).collect();
        let description = gallery.description();

        Ok(CatalogItem {
            key: id.to_string(),
            title: gallery.title,
            url: Some(format!("{BASE_URL}{}", gallery.galleryurl)),
            cover,
            description: Some(description),
            authors,
            artists,
            tags,
            status: Some(json!("completed")),
            initialized: true,
            language: gallery
                .language
                .as_deref()
                .and_then(hitomi_language_code)
                .map(ToOwned::to_owned)
                .or_else(|| Some(self.source_language.to_owned())),
            content_rating: Some("adult".to_owned()),
            update_strategy: Some(json!("onlyFetchOnce")),
            ..CatalogItem::default()
        })
    }

    fn image_config(&mut self) -> Result<&ImageConfig> {
        if self.image_config.is_none() {
            let script = self.text(&format!("{LTN_URL}/gg.js"), 500_000)?;
            self.image_config = Some(parse_image_config(&script)?);
        }
        self.image_config
            .as_ref()
            .ok_or_else(|| Error::new("Hitomi image configuration is unavailable"))
    }

    fn image_request(
        &mut self,
        file: &GalleryFile,
        thumbnail: bool,
        gallery_id: u32,
    ) -> Result<ImageRequest> {
        let config = self.image_config()?.clone();
        let url = config.image_url(file, thumbnail)?;
        Ok(ImageRequest::get(url)
            .header(
                "Accept",
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
            )
            .header("Referer", format!("{BASE_URL}/reader/{gallery_id}.html"))
            .header("Origin", BASE_URL))
    }

    fn index_version(&mut self) -> Result<&str> {
        if self.index_version.is_none() {
            let version = self
                .text(&format!("{LTN_URL}/galleriesindex/version"), 128)?
                .trim()
                .to_owned();
            if version.is_empty()
                || !version
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
            {
                return Err(Error::new(
                    "Hitomi returned an invalid gallery index version",
                ));
            }
            self.index_version = Some(version);
        }
        Ok(self.index_version.as_deref().unwrap_or_default())
    }

    fn gallery_node(&mut self, address: u64) -> Result<Node> {
        let version = self.index_version()?.to_owned();
        let bytes = self.ranged_bytes(
            &format!("{LTN_URL}/galleriesindex/galleries.{version}.index"),
            Some(address),
            NODE_SIZE,
        )?;
        decode_node(&bytes)
    }

    fn query_ids(&mut self, query: &str, language: &str) -> Result<Vec<u32>> {
        let query = query.replace('_', " ");
        if let Some((namespace, raw_tag)) = query.split_once(':') {
            let (area, tag, lang) = match namespace {
                "female" | "male" => (Some("tag"), query.as_str(), language),
                "language" => (None, "index", raw_tag),
                namespace => (Some(namespace), raw_tag, language),
            };
            return self.nozomi_ids(area, tag, lang, None);
        }

        let digest = Sha256::digest(query.as_bytes());
        let key = &digest[..4];
        let mut node = self.gallery_node(0)?;
        loop {
            let (found, position) = node.locate(key);
            if found {
                let data = *node
                    .data
                    .get(position)
                    .ok_or_else(|| Error::new("Hitomi gallery index node is inconsistent"))?;
                return self.gallery_ids_from_data(data);
            }
            if node.is_leaf() {
                return Ok(Vec::new());
            }
            let address = *node
                .subnodes
                .get(position)
                .ok_or_else(|| Error::new("Hitomi gallery index node has no child"))?;
            node = self.gallery_node(address)?;
        }
    }

    fn gallery_ids_from_data(&mut self, data: (u64, u32)) -> Result<Vec<u32>> {
        let (offset, length) = data;
        if length < 4 || u64::from(length) > MAX_SEARCH_DATA_BYTES {
            return Err(Error::new(
                "Hitomi search result is outside the supported size",
            ));
        }
        let version = self.index_version()?.to_owned();
        let bytes = self.ranged_bytes(
            &format!("{LTN_URL}/galleriesindex/galleries.{version}.data"),
            Some(offset),
            u64::from(length),
        )?;
        decode_search_data(&bytes)
    }

    fn search_ids(&mut self, query: &str, filters: &Value) -> Result<Vec<u32>> {
        let sort = selected(filters, "sort").unwrap_or("index");
        let (sort_area, sort_tag, random) = match sort {
            "published" => (Some("date"), "published", false),
            "today" => (Some("popular"), "today", false),
            "week" => (Some("popular"), "week", false),
            "month" => (Some("popular"), "month", false),
            "year" => (Some("popular"), "year", false),
            "random" => (Some("popular"), "year", true),
            _ => (None, "index", false),
        };

        let mut terms = query
            .trim()
            .to_lowercase()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        for (id, namespace) in [
            ("groups", "group"),
            ("artists", "artist"),
            ("series", "series"),
            ("characters", "character"),
            ("male_tags", "male"),
            ("female_tags", "female"),
            ("tags", "tag"),
        ] {
            if let Some(value) = selected(filters, id) {
                terms.extend(value.split(',').filter_map(|term| {
                    let term = term.trim().to_lowercase();
                    if term.is_empty() {
                        None
                    } else if let Some(term) = term.strip_prefix('-') {
                        Some(format!("-{namespace}:{term}"))
                    } else {
                        Some(format!("{namespace}:{term}"))
                    }
                }));
            }
        }

        let enabled_types = selected_values(filters, "types");
        let all_types = [
            "anime",
            "artistcg",
            "doujinshi",
            "gamecg",
            "imageset",
            "manga",
        ];
        if filters.get("types").is_some() {
            let disabled = all_types
                .iter()
                .filter(|kind| !enabled_types.iter().any(|selected| selected == **kind))
                .collect::<Vec<_>>();
            if enabled_types.is_empty() {
                terms.push("type:none".to_owned());
            } else if disabled.len() < 5 {
                terms.extend(disabled.into_iter().map(|kind| format!("-type:{kind}")));
            } else if enabled_types.len() == 1 {
                terms.push(format!("type:{}", enabled_types[0]));
            }
        }

        if self.nozomi_language != "all"
            && sort_area.is_none()
            && sort_tag == "index"
            && !terms.iter().any(|term| term.contains(':'))
        {
            terms.push(format!("language:{}", self.nozomi_language));
        }

        let (positive, negative): (Vec<_>, Vec<_>) = terms
            .into_iter()
            .filter(|term| !term.is_empty())
            .partition(|term| !term.starts_with('-'));

        let mut results = if positive.is_empty() || sort_area.is_some() || sort_tag != "index" {
            Some(self.nozomi_ids(sort_area, sort_tag, self.nozomi_language, None)?)
        } else {
            None
        };

        for term in positive {
            let ids = self.query_ids(&term, self.nozomi_language)?;
            intersect_ordered(&mut results, ids);
        }
        let mut results = results.unwrap_or_default();
        for term in negative {
            let ids = self.query_ids(term.trim_start_matches('-'), self.nozomi_language)?;
            let membership = ids.into_iter().collect::<BTreeSet<_>>();
            results.retain(|id| !membership.contains(id));
        }
        if random {
            results.sort_by_key(|id| {
                let digest = Sha256::digest(format!("{query}:{id}").as_bytes());
                u64::from_be_bytes(digest[..8].try_into().unwrap_or_default())
            });
        }
        Ok(results)
    }
}

impl MangaSource for HitomiSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(Some("popular"), "year", page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(None, "index", page)
    }

    fn listing(
        &mut self,
        listing: &str,
        page: u32,
        _filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.popular(page),
            "latest" => self.latest(page),
            other => Err(Error::new(format!("unknown Hitomi listing {other:?}"))),
        }
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if query.starts_with("https://") || query.starts_with("http://") {
            if let Some(resolved) = self.handle_url(query)? {
                return Ok(Paged::new(resolved.item.into_iter().collect(), false));
            }
        }
        let ids = self.search_ids(query, filters)?;
        let start = (page.max(1) as usize - 1).saturating_mul(PAGE_SIZE);
        let has_next_page = start.saturating_add(PAGE_SIZE) < ids.len();
        let page_ids = ids
            .into_iter()
            .skip(start)
            .take(PAGE_SIZE)
            .collect::<Vec<_>>();
        Ok(Paged::new(self.galleries(&page_ids)?, has_next_page))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let id = item_id(&item)?;
        let gallery = self.gallery(id)?;
        self.gallery_to_item(gallery)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let id = item_id(&item)?;
        let gallery = self.gallery(id)?;
        Ok(vec![MangaChapter {
            key: id.to_string(),
            title: Some("Chapter".to_owned()),
            date_uploaded: parse_date(&gallery.date),
            scanlators: gallery.kind.into_iter().collect(),
            language: gallery
                .language
                .as_deref()
                .and_then(hitomi_language_code)
                .map(ToOwned::to_owned)
                .or_else(|| Some(self.source_language.to_owned())),
            url: Some(format!("{BASE_URL}{}", gallery.galleryurl)),
            source_order: Some(0),
            page_count: Some(gallery.files.len() as u32),
            ..MangaChapter::default()
        }])
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let id = chapter
            .key
            .parse::<u32>()
            .ok()
            .or_else(|| gallery_id_from_path(chapter.url.as_deref().unwrap_or_default()).ok())
            .unwrap_or(item_id(&item)?);
        let gallery = self.gallery(id)?;
        gallery
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let request = self.image_request(file, false, id)?;
                Ok(MangaPage {
                    content: PageContent::Url {
                        url: request.url,
                        context: Some(request.headers),
                    },
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
            })
            .collect()
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filters())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(item.url.clone().or_else(|| {
            item.key
                .parse::<u32>()
                .ok()
                .map(|id| format!("{BASE_URL}/reader/{id}.html"))
        }))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        Ok(chapter.url.clone().or_else(|| {
            chapter
                .key
                .parse::<u32>()
                .ok()
                .map(|id| format!("{BASE_URL}/reader/{id}.html"))
        }))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(url) = Url::parse(candidate) else {
            return Ok(None);
        };
        if url.host_str() != Some("hitomi.la") && url.host_str() != Some("www.hitomi.la") {
            return Ok(None);
        }
        let Ok(id) = gallery_id_from_path(url.path()) else {
            return Ok(None);
        };
        let item = self.gallery_to_item(self.gallery(id)?)?;
        let mut result = UrlResolveResult {
            item: Some(item.clone()),
            ..UrlResolveResult::default()
        };
        if url.path().starts_with("/reader/") {
            result.chapter_key = Some(id.to_string());
            result.manga_chapter = self.chapters(item)?.into_iter().next();
        }
        Ok(Some(result))
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn extension_registry() -> manatan_sdk::Extension {
    LANGUAGE_VARIANTS.into_iter().fold(
        manatan_sdk::Extension::new(),
        |extension, (id, source_language, nozomi_language)| {
            extension.manga(id, HitomiSource::new(source_language, nozomi_language))
        },
    )
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension_registry());

const LANGUAGE_VARIANTS: [(&str, &str, &str); 26] = [
    ("hitomi", "all", "all"),
    ("hitomi-en", "en", "english"),
    ("hitomi-id", "id", "indonesian"),
    ("hitomi-jv", "jv", "javanese"),
    ("hitomi-ca", "ca", "catalan"),
    ("hitomi-ceb", "ceb", "cebuano"),
    ("hitomi-cs", "cs", "czech"),
    ("hitomi-da", "da", "danish"),
    ("hitomi-de", "de", "german"),
    ("hitomi-et", "et", "estonian"),
    ("hitomi-es", "es", "spanish"),
    ("hitomi-eo", "eo", "esperanto"),
    ("hitomi-fr", "fr", "french"),
    ("hitomi-it", "it", "italian"),
    ("hitomi-hi", "hi", "hindi"),
    ("hitomi-hu", "hu", "hungarian"),
    ("hitomi-pl", "pl", "polish"),
    ("hitomi-pt", "pt", "portuguese"),
    ("hitomi-vi", "vi", "vietnamese"),
    ("hitomi-tr", "tr", "turkish"),
    ("hitomi-ru", "ru", "russian"),
    ("hitomi-uk", "uk", "ukrainian"),
    ("hitomi-ar", "ar", "arabic"),
    ("hitomi-ko", "ko", "korean"),
    ("hitomi-zh", "zh", "chinese"),
    ("hitomi-ja", "ja", "japanese"),
];

#[derive(Clone, Debug, Deserialize)]
struct Gallery {
    galleryurl: String,
    title: String,
    #[serde(rename = "japanese_title", alias = "japaneseTitle")]
    japanese_title: Option<String>,
    date: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    language: Option<String>,
    tags: Option<Vec<Tag>>,
    artists: Option<Vec<Artist>>,
    groups: Option<Vec<Group>>,
    characters: Option<Vec<Character>>,
    parodys: Option<Vec<Parody>>,
    files: Vec<GalleryFile>,
}

impl Gallery {
    fn description(&self) -> String {
        let mut lines = Vec::new();
        if let Some(title) = self
            .japanese_title
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            lines.push(format!("Japanese title: {title}"));
        }
        if let Some(values) = self.parodys.as_ref().filter(|values| !values.is_empty()) {
            lines.push(format!(
                "Series: {}",
                values
                    .iter()
                    .map(|value| title_case(&value.parody))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(values) = self.characters.as_ref().filter(|values| !values.is_empty()) {
            lines.push(format!(
                "Characters: {}",
                values
                    .iter()
                    .map(|value| title_case(&value.character))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(kind) = self.kind.as_deref() {
            lines.push(format!("Type: {kind}"));
        }
        lines.push(format!("Pages: {}", self.files.len()));
        if let Some(language) = self.language.as_deref() {
            lines.push(format!("Language: {language}"));
        }
        lines.join("\n")
    }
}

#[derive(Clone, Debug, Deserialize)]
struct GalleryFile {
    hash: String,
    name: String,
}

impl GalleryFile {
    fn uses_webp(&self) -> bool {
        self.name.ends_with(".gif") || self.name.ends_with(".webp")
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Tag {
    tag: String,
    #[serde(default, deserialize_with = "truthy_string")]
    female: bool,
    #[serde(default, deserialize_with = "truthy_string")]
    male: bool,
}

impl Tag {
    fn formatted(&self) -> String {
        let suffix = if self.female {
            " ♀"
        } else if self.male {
            " ♂"
        } else {
            ""
        };
        format!("{}{suffix}", title_case(&self.tag))
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Artist {
    artist: String,
}
#[derive(Clone, Debug, Deserialize)]
struct Group {
    group: String,
}
#[derive(Clone, Debug, Deserialize)]
struct Character {
    character: String,
}
#[derive(Clone, Debug, Deserialize)]
struct Parody {
    parody: String,
}

fn truthy_string<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(
        matches!(value, Some(Value::String(ref value)) if value == "1")
            || matches!(value, Some(Value::Number(ref value)) if value.as_u64() == Some(1))
            || matches!(value, Some(Value::Bool(true))),
    )
}

#[derive(Clone, Debug)]
struct ImageConfig {
    default_offset: u32,
    mapped_offset: u32,
    mapped_ids: BTreeSet<u32>,
    common_id: String,
}

impl ImageConfig {
    fn image_url(&self, file: &GalleryFile, thumbnail: bool) -> Result<String> {
        let image_id = image_id_from_hash(&file.hash)?;
        let offset = if self.mapped_ids.contains(&image_id) {
            self.mapped_offset
        } else {
            self.default_offset
        };
        let extension = if file.uses_webp() { "webp" } else { "avif" };
        if thumbnail {
            let subdomain = char::from_u32(u32::from(b'a') + offset)
                .ok_or_else(|| Error::new("Hitomi image subdomain is invalid"))?;
            let path = thumbnail_path(&file.hash)?;
            return Ok(format!(
                "https://{subdomain}tn.{CDN_DOMAIN}/{extension}bigtn/{path}/{}.{extension}",
                file.hash
            ));
        }
        let prefix = if file.uses_webp() { "w" } else { "a" };
        Ok(format!(
            "https://{prefix}{}.{CDN_DOMAIN}/{}{image_id}/{}.{extension}",
            offset + 1,
            self.common_id,
            file.hash
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    keys: Vec<Vec<u8>>,
    data: Vec<(u64, u32)>,
    subnodes: Vec<u64>,
}

impl Node {
    fn locate(&self, key: &[u8]) -> (bool, usize) {
        for (index, candidate) in self.keys.iter().enumerate() {
            if key <= candidate.as_slice() {
                return (key == candidate.as_slice(), index);
            }
        }
        (false, self.keys.len())
    }

    fn is_leaf(&self) -> bool {
        self.subnodes.iter().all(|address| *address == 0)
    }
}

fn parse_gallery_script(script: &str) -> Result<Gallery> {
    let json = script
        .split_once("var galleryinfo = ")
        .map(|(_, json)| json.trim().trim_end_matches(';'))
        .ok_or_else(|| Error::new("Hitomi gallery response has no galleryinfo"))?;
    serde_json::from_str(json).map_err(Error::from)
}

fn parse_image_config(script: &str) -> Result<ImageConfig> {
    let default_offset = capture_u32(script, r"var o = (\d+)")?;
    let mapped_offset = capture_u32(script, r"o = (\d+); break;")?;
    let mapped_ids = Regex::new(r"case (\d+):")
        .map_err(|error| Error::new(error.to_string()))?
        .captures_iter(script)
        .filter_map(|capture| capture.get(1)?.as_str().parse().ok())
        .collect();
    let common_id = Regex::new(r"b: '([^']+)'")
        .map_err(|error| Error::new(error.to_string()))?
        .captures(script)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("Hitomi gg.js has no common image id"))?;
    Ok(ImageConfig {
        default_offset,
        mapped_offset,
        mapped_ids,
        common_id,
    })
}

fn capture_u32(value: &str, pattern: &str) -> Result<u32> {
    Regex::new(pattern)
        .map_err(|error| Error::new(error.to_string()))?
        .captures(value)
        .and_then(|capture| capture.get(1))
        .and_then(|capture| capture.as_str().parse().ok())
        .ok_or_else(|| Error::new("Hitomi gg.js has an unsupported format"))
}

fn image_id_from_hash(hash: &str) -> Result<u32> {
    if hash.len() < 3 || !hash.is_ascii() {
        return Err(Error::new("Hitomi image hash is invalid"));
    }
    let suffix = &hash[hash.len() - 3..];
    u32::from_str_radix(&format!("{}{}", &suffix[2..], &suffix[..2]), 16)
        .map_err(|_| Error::new("Hitomi image hash suffix is invalid"))
}

fn thumbnail_path(hash: &str) -> Result<String> {
    if hash.len() < 3 || !hash.is_ascii() {
        return Err(Error::new("Hitomi image hash is invalid"));
    }
    let suffix = &hash[hash.len() - 3..];
    Ok(format!("{}/{}", &suffix[2..], &suffix[..2]))
}

fn decode_u32_list(bytes: &[u8]) -> Result<Vec<u32>> {
    let (chunks, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return Err(Error::new(
            "Hitomi nozomi response is not aligned to 32-bit ids",
        ));
    }
    Ok(chunks.iter().copied().map(u32::from_be_bytes).collect())
}

fn decode_search_data(bytes: &[u8]) -> Result<Vec<u32>> {
    if bytes.len() < 4 {
        return Err(Error::new("Hitomi search data is truncated"));
    }
    let count = u32::from_be_bytes(bytes[..4].try_into().unwrap_or_default()) as usize;
    let expected = count
        .checked_mul(4)
        .and_then(|length| length.checked_add(4))
        .ok_or_else(|| Error::new("Hitomi search result size overflow"))?;
    if expected != bytes.len() || count > 10_000_000 {
        return Err(Error::new(
            "Hitomi search data has an invalid gallery count",
        ));
    }
    decode_u32_list(&bytes[4..])
}

fn decode_node(bytes: &[u8]) -> Result<Node> {
    if bytes.len() != NODE_SIZE as usize {
        return Err(Error::new("Hitomi gallery index node is truncated"));
    }
    let mut cursor = Cursor::new(bytes);
    let key_count = cursor.u32()? as usize;
    if key_count > 16 {
        return Err(Error::new("Hitomi gallery index node has too many keys"));
    }
    let mut keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        let length = cursor.u32()? as usize;
        if !(1..=32).contains(&length) {
            return Err(Error::new("Hitomi gallery index key has an invalid size"));
        }
        keys.push(cursor.bytes(length)?.to_vec());
    }
    let data_count = cursor.u32()? as usize;
    if data_count > 16 || data_count != key_count {
        return Err(Error::new(
            "Hitomi gallery index node has inconsistent data",
        ));
    }
    let mut data = Vec::with_capacity(data_count);
    for _ in 0..data_count {
        data.push((cursor.u64()?, cursor.u32()?));
    }
    let mut subnodes = Vec::with_capacity(17);
    for _ in 0..17 {
        subnodes.push(cursor.u64()?);
    }
    Ok(Node {
        keys,
        data,
        subnodes,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn bytes(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| Error::new("Hitomi index cursor overflow"))?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| Error::new("Hitomi gallery index node is truncated"))?;
        self.position = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.bytes(4)?.try_into().unwrap_or_default(),
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.bytes(8)?.try_into().unwrap_or_default(),
        ))
    }
}

fn filters() -> Vec<FilterDefinition> {
    vec![
        select_filter(
            "sort",
            "Sort by",
            &[
                ("Date added", "index"),
                ("Date published", "published"),
                ("Popular: Today", "today"),
                ("Popular: Week", "week"),
                ("Popular: Month", "month"),
                ("Popular: Year", "year"),
                ("Random", "random"),
            ],
        ),
        FilterDefinition::MultiSelect {
            id: "types".to_owned(),
            name: "Types".to_owned(),
            options: [
                ("Anime", "anime"),
                ("Artist CG", "artistcg"),
                ("Doujinshi", "doujinshi"),
                ("Game CG", "gamecg"),
                ("Image Set", "imageset"),
                ("Manga", "manga"),
            ]
            .into_iter()
            .map(|(label, value)| OptionItem {
                label: label.to_owned(),
                value: value.to_owned(),
            })
            .collect(),
            default: vec![
                "anime",
                "artistcg",
                "doujinshi",
                "gamecg",
                "imageset",
                "manga",
            ]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
        },
        FilterDefinition::Header {
            name: "Separate tags with commas. Prefix a tag with - to exclude it.".to_owned(),
        },
        text_filter("groups", "Groups"),
        text_filter("artists", "Artists"),
        text_filter("series", "Series"),
        text_filter("characters", "Characters"),
        text_filter("male_tags", "Male tags"),
        text_filter("female_tags", "Female tags"),
        text_filter("tags", "Other tags"),
    ]
}

fn select_filter(id: &str, name: &str, entries: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.to_owned(),
        name: name.to_owned(),
        options: entries
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).to_owned(),
                value: (*value).to_owned(),
            })
            .collect(),
        default_index: 0,
    }
}

fn text_filter(id: &str, name: &str) -> FilterDefinition {
    FilterDefinition::Text {
        id: id.to_owned(),
        name: name.to_owned(),
        default: String::new(),
    }
}

fn selected<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn selected_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
}

fn item_id(item: &CatalogItem) -> Result<u32> {
    item.key
        .parse()
        .ok()
        .or_else(|| {
            item.url
                .as_deref()
                .and_then(|url| gallery_id_from_path(url).ok())
        })
        .ok_or_else(|| Error::new("Hitomi item has no gallery id"))
}

fn gallery_id_from_path(value: &str) -> Result<u32> {
    let path = Url::parse(value)
        .ok()
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|| value.to_owned());
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".html");
    let id = stem.rsplit('-').next().unwrap_or(stem);
    id.parse()
        .map_err(|_| Error::new("Hitomi URL has no gallery id"))
}

fn parse_date(value: &str) -> Option<i64> {
    DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%:z")
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%#z"))
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|date| date.timestamp_millis())
}

fn intersect_ordered(current: &mut Option<Vec<u32>>, next: Vec<u32>) {
    if let Some(current) = current {
        let membership = next.into_iter().collect::<BTreeSet<_>>();
        current.retain(|id| membership.contains(id));
    } else {
        *current = Some(next);
    }
}

fn title_case(value: &str) -> String {
    let mut capitalize = true;
    value
        .chars()
        .map(|character| {
            let result = if capitalize {
                character.to_ascii_uppercase()
            } else {
                character.to_ascii_lowercase()
            };
            capitalize = character.is_whitespace();
            result
        })
        .collect()
}

fn hitomi_language_code(language: &str) -> Option<&'static str> {
    LANGUAGE_VARIANTS.iter().find_map(|(_, source, nozomi)| {
        (*nozomi == language || *source == language).then_some(*source)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_script_parses_current_shape() {
        let script = r#"var galleryinfo = {"galleryurl":"/manga/sample-123.html","title":"Sample","japanese_title":"見本","date":"2026-09-03 20:23:00-05:00","type":"manga","language":"japanese","tags":[{"tag":"full color","female":"1","male":""}],"artists":[{"artist":"sample artist"}],"groups":null,"characters":[],"parodys":[],"files":[{"hash":"abcdef123","name":"1.jpg"}]}"#;
        let gallery = parse_gallery_script(script).unwrap();
        assert_eq!(gallery_id_from_path(&gallery.galleryurl).unwrap(), 123);
        assert_eq!(gallery.tags.unwrap()[0].formatted(), "Full Color ♀");
    }

    #[test]
    fn image_config_and_urls_match_protocol() {
        let config = parse_image_config("var o = 1; case 786: o = 0; break; b: '123/'").unwrap();
        let file = GalleryFile {
            hash: "abcdef123".to_owned(),
            name: "1.jpg".to_owned(),
        };
        assert_eq!(image_id_from_hash(&file.hash).unwrap(), 0x312);
        assert_eq!(thumbnail_path(&file.hash).unwrap(), "3/12");
        assert_eq!(
            config.image_url(&file, true).unwrap(),
            "https://atn.gold-usergeneratedcontent.net/avifbigtn/3/12/abcdef123.avif"
        );
        assert_eq!(
            config.image_url(&file, false).unwrap(),
            "https://a1.gold-usergeneratedcontent.net/123/786/abcdef123.avif"
        );
    }

    #[test]
    fn decodes_big_endian_gallery_ids() {
        assert_eq!(
            decode_u32_list(&[0, 0, 0, 1, 0, 0, 1, 0]).unwrap(),
            vec![1, 256]
        );
        assert!(decode_u32_list(&[0, 1]).is_err());
    }

    #[test]
    fn url_resolution_extracts_gallery_and_reader_ids() {
        assert_eq!(
            gallery_id_from_path("https://hitomi.la/manga/example-42.html").unwrap(),
            42
        );
        assert_eq!(gallery_id_from_path("/reader/42.html").unwrap(), 42);
    }

    #[test]
    fn every_upstream_language_has_a_distinct_source() {
        assert_eq!(LANGUAGE_VARIANTS.len(), 26);
        assert_eq!(
            LANGUAGE_VARIANTS
                .iter()
                .map(|entry| entry.0)
                .collect::<BTreeSet<_>>()
                .len(),
            26
        );
        let _registry = extension_registry();
    }

    #[test]
    fn search_data_checks_declared_length() {
        assert_eq!(
            decode_search_data(&[0, 0, 0, 2, 0, 0, 0, 7, 0, 0, 0, 9]).unwrap(),
            vec![7, 9]
        );
        assert!(decode_search_data(&[0, 0, 0, 2, 0, 0, 0, 7]).is_err());
    }

    #[test]
    fn ordered_intersection_keeps_an_empty_first_result_empty() {
        let mut result = None;
        intersect_ordered(&mut result, Vec::new());
        intersect_ordered(&mut result, vec![1, 2, 3]);
        assert_eq!(result, Some(Vec::new()));
    }

    #[test]
    fn parses_hitomis_short_timezone_offset() {
        assert!(parse_date("2026-09-03 20:23:00-05").is_some());
    }
}
