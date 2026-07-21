use std::collections::{BTreeMap, BTreeSet};

use chrono::DateTime;
use manatan_sdk::{
    client::Client, context, CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter,
    MangaPage, MangaPageImage, MangaSource, OptionItem, PageContent, Paged, PreferenceDefinition,
    Result, UrlResolveResult,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

const SITE_URL: &str = "https://mangadex.org";
const API_URL: &str = "https://api.mangadex.org";
const UPLOADS_URL: &str = "https://uploads.mangadex.org";
const LIST_LIMIT: u32 = 20;
const LATEST_LIMIT: u32 = 50;
const FEED_LIMIT: u32 = 500;
const REQUEST_LIMIT_MS: u32 = 350;
const NATIVE_USER_AGENT: &str = "Manatan/6.0 (+https://manatan.com)";

const LANGUAGE_KEY: &str = "translated_language";
const ORIGINAL_LANGUAGES_KEY: &str = "original_languages";
const COVER_QUALITY_KEY: &str = "cover_quality";
const DATA_SAVER_KEY: &str = "data_saver";
const FORCE_PORT_443_KEY: &str = "force_port_443";
const BLOCKED_GROUPS_KEY: &str = "blocked_groups";
const BLOCKED_UPLOADERS_KEY: &str = "blocked_uploaders";

const DEFAULT_BLOCKED_GROUPS: &[&str] = &[
    "5fed0576-8b94-4f9a-b6a7-08eecd69800d",
    "06a9fecb-b608-4f19-b93c-7caab06b7f44",
    "8d8ecf83-8d42-4f8c-add8-60963f9f28d9",
    "caa63201-4a17-4b7f-95ff-ed884a2b7e60",
    "319c1b10-cbd0-4f55-a46e-c4ee17e65139",
    "4f1de6a2-f0c5-4ac5-bce5-02c7dbb67deb",
];

pub struct MangaDexSource {
    client: Client,
    allowed_manga: BTreeSet<String>,
}

impl Default for MangaDexSource {
    fn default() -> Self {
        Self {
            // MangaDex's API rejects browser-identifying User-Agent values on
            // API requests. Identify the native client explicitly and send
            // only API-specific headers here.
            client: Client::new()
                .header("User-Agent", NATIVE_USER_AGENT)
                .header("Accept", "application/json")
                .header("Referer", format!("{SITE_URL}/"))
                .header("Origin", SITE_URL),
            allowed_manga: BTreeSet::new(),
        }
    }
}

impl MangaDexSource {
    fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self
            .client
            .get(url)
            .rate_limit("mangadex", REQUEST_LIMIT_MS)
            .send()?
            .error_for_status()?;
        serde_json::from_str(response.text()?)
            .map_err(|error| Error::new(format!("MangaDex JSON parse error: {error}")))
    }

    fn manga_entity(&mut self, manga_id: &str) -> Result<MangaData> {
        let manga_id = require_uuid(manga_id, "manga")?;
        let mut url = Url::parse(&format!("{API_URL}/manga/{manga_id}")).map_err(url_error)?;
        url.query_pairs_mut()
            .append_pair("includes[]", "cover_art")
            .append_pair("includes[]", "author")
            .append_pair("includes[]", "artist");
        let response: EntityResponse<MangaData> = self.get_json(url.as_str())?;
        ensure_allowed_rating(response.data.attributes.content_rating.as_deref())?;
        self.allowed_manga.insert(response.data.id.clone());
        Ok(response.data)
    }

    fn aggregate(&self, manga_id: &str) -> Result<AggregateResponse> {
        let mut url = Url::parse(&format!(
            "{API_URL}/manga/{}/aggregate",
            require_uuid(manga_id, "manga")?
        ))
        .map_err(url_error)?;
        url.query_pairs_mut()
            .append_pair("translatedLanguage[]", &translated_language());
        self.get_json(url.as_str())
    }

    fn mangas_by_ids(&mut self, ids: &[String]) -> Result<Vec<MangaData>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut url = Url::parse(&format!("{API_URL}/manga")).map_err(url_error)?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("limit", &ids.len().min(100).to_string())
                .append_pair("includes[]", "cover_art");
            append_safe_ratings_inline(&mut query);
            for id in ids.iter().take(100) {
                query.append_pair("ids[]", require_uuid(id, "manga")?);
            }
        }
        let response: CollectionResponse<MangaData> = self.get_json(url.as_str())?;
        let entries = response
            .data
            .into_iter()
            .filter(|manga| rating_is_allowed(manga.attributes.content_rating.as_deref()))
            .collect::<Vec<_>>();
        self.allowed_manga
            .extend(entries.iter().map(|manga| manga.id.clone()));
        Ok(entries)
    }

    fn at_home(&self, chapter_id: &str) -> Result<AtHomeResponse> {
        let mut url = Url::parse(&format!(
            "{API_URL}/at-home/server/{}",
            require_uuid(chapter_id, "chapter")?
        ))
        .map_err(url_error)?;
        if force_port_443() {
            url.query_pairs_mut().append_pair("forcePort443", "true");
        }
        let response: AtHomeResponse = self.get_json(url.as_str())?;
        validate_at_home_base(&response.base_url)?;
        Ok(response)
    }

    fn ensure_item_allowed(&mut self, item: &CatalogItem) -> Result<String> {
        let id = manga_id(item.url.as_deref().unwrap_or(&item.key))?;
        if !self.allowed_manga.contains(&id) {
            self.manga_entity(&id)?;
        }
        Ok(id)
    }
}

impl MangaSource for MangaDexSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = manga_list_url(page, Some(("followedCount", "desc")), "", &Value::Null)?;
        let response: CollectionResponse<MangaData> = self.get_json(&url)?;
        parse_catalog(response)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = latest_feed_url(page)?;
        let response: CollectionResponse<ChapterData> = self.get_json(&url)?;
        let mut ids = Vec::new();
        let mut seen = BTreeSet::new();
        for chapter in &response.data {
            if let Some(id) = relationship_id(&chapter.relationships, "manga") {
                if seen.insert(id.to_owned()) {
                    ids.push(id.to_owned());
                }
            }
        }
        let manga = self.mangas_by_ids(&ids)?;
        let mut by_id = manga
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        let entries = ids
            .iter()
            .filter_map(|id| by_id.remove(id))
            .map(|entry| entry.to_catalog_item(&translated_language(), &cover_quality()))
            .collect::<Result<Vec<_>>>()?;
        Ok(Paged {
            entries,
            has_next_page: response.offset.saturating_add(response.limit) < response.total,
        })
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let trimmed = query.trim();
        if trimmed.starts_with("https://") {
            if let Some(resolved) = self.handle_url(trimmed)? {
                if let Some(item) = resolved.item {
                    return Ok(Paged::new(vec![item], false));
                }
            }
        }
        let url = manga_list_url(page, None, trimmed, filters)?;
        let response: CollectionResponse<MangaData> = self.get_json(&url)?;
        parse_catalog(response)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let manga_id = manga_id(item.url.as_deref().unwrap_or(&item.key))?;
        let manga = self.manga_entity(&manga_id)?;
        let mut parsed = manga.to_catalog_item(&translated_language(), &cover_quality())?;
        parsed.initialized = true;
        if let Ok(aggregate) = self.aggregate(&manga_id) {
            let (volumes, chapters) = aggregate_stats(&aggregate);
            parsed
                .extra
                .insert("aggregateVolumes".to_owned(), Value::from(volumes));
            parsed
                .extra
                .insert("aggregateChapters".to_owned(), Value::from(chapters));
        }
        Ok(parsed)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let manga_id = self.ensure_item_allowed(&item)?;
        let mut offset = 0_u32;
        let mut source_order = 0_i32;
        let mut chapters = Vec::new();
        loop {
            let url = chapter_feed_url(&manga_id, offset)?;
            let response: CollectionResponse<ChapterData> = self.get_json(&url)?;
            let count = response.data.len() as u32;
            for chapter in response.data {
                if let Some(chapter) = chapter.to_manga_chapter(source_order) {
                    chapters.push(chapter);
                    source_order += 1;
                }
            }
            offset = offset.saturating_add(response.limit.max(count));
            if count == 0 || offset >= response.total {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        self.ensure_item_allowed(&item)?;
        let chapter_id = chapter_id(chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let at_home = self.at_home(&chapter_id)?;
        pages_from_at_home(&chapter_id, &at_home, data_saver())
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filter_definitions())
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(preference_definitions())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(format!(
            "{SITE_URL}/title/{}",
            manga_id(item.url.as_deref().unwrap_or(&item.key))?
        )))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        Ok(Some(format!(
            "{SITE_URL}/chapter/{}",
            chapter_id(chapter.url.as_deref().unwrap_or(&chapter.key))?
        )))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some((kind, id)) = supported_deep_link(candidate)? else {
            return Ok(None);
        };
        match kind {
            "title" | "manga" => {
                let item = self.details(CatalogItem::new(id.clone(), ""))?;
                Ok(Some(UrlResolveResult {
                    item: Some(item),
                    ..UrlResolveResult::default()
                }))
            }
            "chapter" => {
                let mut url = Url::parse(&format!("{API_URL}/chapter/{id}")).map_err(url_error)?;
                url.query_pairs_mut()
                    .append_pair("includes[]", "manga")
                    .append_pair("includes[]", "scanlation_group");
                let response: EntityResponse<ChapterData> = self.get_json(url.as_str())?;
                let manga_id = relationship_id(&response.data.relationships, "manga")
                    .ok_or_else(|| Error::new("MangaDex chapter has no manga relationship"))?;
                let item = self.details(CatalogItem::new(manga_id, ""))?;
                let manga_chapter = response
                    .data
                    .to_manga_chapter(0)
                    .ok_or_else(|| Error::new("MangaDex chapter is unavailable or external"))?;
                Ok(Some(UrlResolveResult {
                    item: Some(item),
                    chapter_key: Some(manga_chapter.key.clone()),
                    manga_chapter: Some(manga_chapter),
                    ..UrlResolveResult::default()
                }))
            }
            _ => Ok(None),
        }
    }

    fn resolve_page_image(
        &mut self,
        item: &CatalogItem,
        _chapter: &MangaChapter,
        page: &MangaPage,
    ) -> Result<Option<MangaPageImage>> {
        self.ensure_item_allowed(item)?;
        let chapter_id = page
            .extra
            .get("chapterId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("MangaDex lazy page is missing chapterId"))?;
        let index = page
            .extra
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("MangaDex lazy page is missing index"))?
            as usize;
        let saver = page
            .extra
            .get("dataSaver")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let refreshed = self.at_home(chapter_id)?;
        let url = at_home_image_url(&refreshed, index, saver)?;
        Ok(Some(MangaPageImage {
            url,
            headers: image_headers(),
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("mangadex", MangaDexSource::default())
);

#[derive(Clone, Debug, Default, Deserialize)]
struct CollectionResponse<T> {
    #[serde(default)]
    data: Vec<T>,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    offset: u32,
    #[serde(default)]
    total: u32,
}

#[derive(Clone, Debug, Deserialize)]
struct EntityResponse<T> {
    data: T,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaData {
    id: String,
    #[serde(default)]
    attributes: MangaAttributes,
    #[serde(default)]
    relationships: Vec<Relationship>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaAttributes {
    #[serde(default)]
    title: BTreeMap<String, String>,
    #[serde(default)]
    alt_titles: Vec<BTreeMap<String, String>>,
    #[serde(default)]
    description: BTreeMap<String, String>,
    original_language: Option<String>,
    last_volume: Option<String>,
    last_chapter: Option<String>,
    publication_demographic: Option<String>,
    status: Option<String>,
    content_rating: Option<String>,
    #[serde(default)]
    tags: Vec<TagData>,
}

impl MangaData {
    fn to_catalog_item(
        &self,
        preferred_language: &str,
        cover_quality: &str,
    ) -> Result<CatalogItem> {
        ensure_allowed_rating(self.attributes.content_rating.as_deref())?;
        let title = localized_title(self, preferred_language);
        let cover = relationship_attribute(&self.relationships, "cover_art", "fileName")
            .map(|file| cover_request(&self.id, file, cover_quality));
        let description = localized_value(
            &self.attributes.description,
            preferred_language,
            self.attributes.original_language.as_deref(),
        );
        let authors = relationship_names(&self.relationships, "author");
        let artists = relationship_names(&self.relationships, "artist");
        let tags = self
            .attributes
            .tags
            .iter()
            .filter_map(|tag| localized_value(&tag.attributes.name, preferred_language, Some("en")))
            .collect();
        let content_rating = self
            .attributes
            .content_rating
            .clone()
            .unwrap_or_else(|| "safe".to_owned());
        let mut extra = BTreeMap::new();
        extra.insert(
            "originalLanguage".to_owned(),
            Value::from(
                self.attributes
                    .original_language
                    .clone()
                    .unwrap_or_default(),
            ),
        );
        if let Some(value) = &self.attributes.last_volume {
            extra.insert("lastVolume".to_owned(), Value::from(value.clone()));
        }
        if let Some(value) = &self.attributes.last_chapter {
            extra.insert("lastChapter".to_owned(), Value::from(value.clone()));
        }
        if let Some(value) = &self.attributes.publication_demographic {
            extra.insert(
                "publicationDemographic".to_owned(),
                Value::from(value.clone()),
            );
        }
        Ok(CatalogItem {
            key: self.id.clone(),
            title,
            url: Some(format!("{SITE_URL}/title/{}", self.id)),
            cover,
            description,
            authors,
            artists,
            tags,
            status: self.attributes.status.clone().map(Value::from),
            initialized: false,
            language: Some(preferred_language.to_owned()),
            content_rating: Some(content_rating),
            extra,
            ..CatalogItem::default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct TagData {
    #[serde(default)]
    attributes: TagAttributes,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct TagAttributes {
    #[serde(default)]
    name: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Relationship {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterData {
    id: String,
    #[serde(default)]
    attributes: ChapterAttributes,
    #[serde(default)]
    relationships: Vec<Relationship>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterAttributes {
    title: Option<String>,
    volume: Option<String>,
    chapter: Option<String>,
    translated_language: Option<String>,
    external_url: Option<String>,
    #[serde(default)]
    is_unavailable: bool,
    publish_at: Option<String>,
    #[serde(default)]
    pages: u32,
}

impl ChapterData {
    fn to_manga_chapter(&self, source_order: i32) -> Option<MangaChapter> {
        if self.attributes.external_url.is_some()
            || self.attributes.pages == 0
            || self.attributes.is_unavailable
        {
            return None;
        }
        let scanlators = relationship_names(&self.relationships, "scanlation_group");
        Some(MangaChapter {
            key: self.id.clone(),
            title: self
                .attributes
                .title
                .clone()
                .filter(|title| !title.trim().is_empty()),
            chapter_number: parse_number(self.attributes.chapter.as_deref()),
            volume_number: parse_number(self.attributes.volume.as_deref()),
            date_uploaded: self.attributes.publish_at.as_deref().and_then(parse_date),
            scanlators,
            language: self.attributes.translated_language.clone(),
            url: Some(format!("{SITE_URL}/chapter/{}", self.id)),
            source_order: Some(source_order),
            page_count: Some(self.attributes.pages),
            extra: [("chapterId".to_owned(), Value::from(self.id.clone()))]
                .into_iter()
                .collect(),
            ..MangaChapter::default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtHomeResponse {
    base_url: String,
    #[serde(default)]
    chapter: AtHomeChapter,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtHomeChapter {
    hash: String,
    #[serde(default)]
    data: Vec<String>,
    #[serde(default)]
    data_saver: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AggregateResponse {
    #[serde(default)]
    volumes: BTreeMap<String, AggregateVolume>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AggregateVolume {
    #[serde(default)]
    chapters: BTreeMap<String, Value>,
}

fn manga_list_url(
    page: u32,
    forced_sort: Option<(&str, &str)>,
    query: &str,
    filters: &Value,
) -> Result<String> {
    let mut url = Url::parse(&format!("{API_URL}/manga")).map_err(url_error)?;
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("limit", &LIST_LIMIT.to_string())
        .append_pair(
            "offset",
            &page
                .max(1)
                .saturating_sub(1)
                .saturating_mul(LIST_LIMIT)
                .to_string(),
        )
        .append_pair("includes[]", "cover_art")
        .append_pair("availableTranslatedLanguage[]", &translated_language());
    append_safe_ratings_inline(&mut pairs);

    for language in selected_original_languages(filters) {
        pairs.append_pair("originalLanguage[]", &language);
    }

    let trimmed = query.trim();
    if let Some(id) = trimmed.strip_prefix("id:") {
        pairs.append_pair("ids[]", require_uuid(id.trim(), "manga")?);
    } else if let Some(id) = trimmed.strip_prefix("grp:") {
        pairs.append_pair("group", require_uuid(id.trim(), "group")?);
    } else if let Some(id) = trimmed.strip_prefix("author:") {
        pairs.append_pair("authorOrArtist", require_uuid(id.trim(), "author")?);
    } else if !trimmed.is_empty() {
        pairs.append_pair("title", trimmed);
    }

    for status in group_values(filters, "statuses") {
        if matches!(
            status.as_str(),
            "ongoing" | "completed" | "hiatus" | "cancelled"
        ) {
            pairs.append_pair("status[]", &status);
        }
    }
    for demographic in group_values(filters, "demographics") {
        if matches!(
            demographic.as_str(),
            "shounen" | "shoujo" | "josei" | "seinen" | "none"
        ) {
            pairs.append_pair("publicationDemographic[]", &demographic);
        }
    }
    for tag in uuid_list(text_value(filters, "included_tags").as_deref()) {
        pairs.append_pair("includedTags[]", &tag);
    }
    for tag in uuid_list(text_value(filters, "excluded_tags").as_deref()) {
        pairs.append_pair("excludedTags[]", &tag);
    }
    let included_mode = select_value(filters, "included_tags_mode").unwrap_or("AND");
    let excluded_mode = select_value(filters, "excluded_tags_mode").unwrap_or("OR");
    pairs.append_pair(
        "includedTagsMode",
        if included_mode.eq_ignore_ascii_case("OR") {
            "OR"
        } else {
            "AND"
        },
    );
    pairs.append_pair(
        "excludedTagsMode",
        if excluded_mode.eq_ignore_ascii_case("AND") {
            "AND"
        } else {
            "OR"
        },
    );
    if let Some(year) = text_value(filters, "year")
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|year| (1900..=2200).contains(year))
    {
        pairs.append_pair("year", &year.to_string());
    }

    let (field, direction) = forced_sort.unwrap_or_else(|| {
        let default = if trimmed.is_empty() {
            "followedCount:desc"
        } else {
            "relevance:desc"
        };
        select_value(filters, "sort")
            .unwrap_or(default)
            .split_once(':')
            .unwrap_or(("relevance", "desc"))
    });
    let field = match field {
        "relevance"
        | "latestUploadedChapter"
        | "followedCount"
        | "createdAt"
        | "updatedAt"
        | "title"
        | "year"
        | "rating" => field,
        _ => "relevance",
    };
    let direction = if direction.eq_ignore_ascii_case("asc") {
        "asc"
    } else {
        "desc"
    };
    pairs.append_pair(&format!("order[{field}]"), direction);
    drop(pairs);
    Ok(url.to_string())
}

fn latest_feed_url(page: u32) -> Result<String> {
    let mut url = Url::parse(&format!("{API_URL}/chapter")).map_err(url_error)?;
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("limit", &LATEST_LIMIT.to_string())
        .append_pair(
            "offset",
            &page
                .max(1)
                .saturating_sub(1)
                .saturating_mul(LATEST_LIMIT)
                .to_string(),
        )
        .append_pair("translatedLanguage[]", &translated_language())
        .append_pair("order[publishAt]", "desc")
        .append_pair("includeFutureUpdates", "0")
        .append_pair("includeFuturePublishAt", "0")
        .append_pair("includeEmptyPages", "0")
        .append_pair("includes[]", "manga");
    append_safe_ratings_inline(&mut pairs);
    for id in blocked_groups() {
        pairs.append_pair("excludedGroups[]", &id);
    }
    for id in blocked_uploaders() {
        pairs.append_pair("excludedUploaders[]", &id);
    }
    drop(pairs);
    Ok(url.to_string())
}

fn chapter_feed_url(manga_id: &str, offset: u32) -> Result<String> {
    let mut url = Url::parse(&format!(
        "{API_URL}/manga/{}/feed",
        require_uuid(manga_id, "manga")?
    ))
    .map_err(url_error)?;
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("limit", &FEED_LIMIT.to_string())
        .append_pair("offset", &offset.to_string())
        .append_pair("translatedLanguage[]", &translated_language())
        .append_pair("order[volume]", "desc")
        .append_pair("order[chapter]", "desc")
        .append_pair("includes[]", "scanlation_group")
        .append_pair("includes[]", "user")
        .append_pair("includeEmptyPages", "0")
        .append_pair("includeFuturePublishAt", "0");
    append_safe_ratings_inline(&mut pairs);
    for id in blocked_groups() {
        pairs.append_pair("excludedGroups[]", &id);
    }
    for id in blocked_uploaders() {
        pairs.append_pair("excludedUploaders[]", &id);
    }
    drop(pairs);
    Ok(url.to_string())
}

fn append_safe_ratings_inline(pairs: &mut url::form_urlencoded::Serializer<'_, url::UrlQuery<'_>>) {
    pairs
        .append_pair("contentRating[]", "safe")
        .append_pair("contentRating[]", "suggestive");
}

fn parse_catalog(response: CollectionResponse<MangaData>) -> Result<Paged<CatalogItem>> {
    let language = translated_language();
    let quality = cover_quality();
    let entries = response
        .data
        .into_iter()
        .filter(|manga| rating_is_allowed(manga.attributes.content_rating.as_deref()))
        .map(|manga| manga.to_catalog_item(&language, &quality))
        .collect::<Result<Vec<_>>>()?;
    Ok(Paged {
        entries,
        has_next_page: response.offset.saturating_add(response.limit) < response.total,
    })
}

fn pages_from_at_home(
    chapter_id: &str,
    response: &AtHomeResponse,
    saver: bool,
) -> Result<Vec<MangaPage>> {
    require_uuid(chapter_id, "chapter")?;
    validate_at_home_base(&response.base_url)?;
    let files = at_home_files(response, saver);
    files
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let url = at_home_image_url(response, index, saver)?;
            let headers = image_headers();
            Ok(MangaPage {
                content: PageContent::Lazy {
                    key: format!("mangadex:{chapter_id}:{index}:{saver}"),
                    url: Some(url),
                    page_url: Some(format!("{API_URL}/at-home/server/{chapter_id}")),
                    context: Some(headers.clone()),
                },
                description: Some(format!("Page {}", index + 1)),
                headers,
                extra: [
                    ("chapterId".to_owned(), Value::from(chapter_id.to_owned())),
                    ("index".to_owned(), Value::from(index as u64)),
                    ("dataSaver".to_owned(), Value::from(saver)),
                ]
                .into_iter()
                .collect(),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn at_home_files(response: &AtHomeResponse, saver: bool) -> &[String] {
    if saver && !response.chapter.data_saver.is_empty() {
        &response.chapter.data_saver
    } else {
        &response.chapter.data
    }
}

fn at_home_image_url(response: &AtHomeResponse, index: usize, saver: bool) -> Result<String> {
    validate_at_home_base(&response.base_url)?;
    if response.chapter.hash.is_empty()
        || response.chapter.hash.contains('/')
        || response.chapter.hash.contains("..")
    {
        return Err(Error::new("MangaDex@Home returned an invalid chapter hash"));
    }
    let files = at_home_files(response, saver);
    let file = files
        .get(index)
        .ok_or_else(|| Error::new("MangaDex@Home page index is out of range"))?;
    if file.is_empty() || file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(Error::new(
            "MangaDex@Home returned an invalid page filename",
        ));
    }
    let mode = if saver && !response.chapter.data_saver.is_empty() {
        "data-saver"
    } else {
        "data"
    };
    Ok(format!(
        "{}/{}/{}/{}",
        response.base_url.trim_end_matches('/'),
        mode,
        response.chapter.hash,
        file
    ))
}

fn validate_at_home_base(candidate: &str) -> Result<()> {
    let url = Url::parse(candidate).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "uploads.mangadex.org"
        || host
            .strip_suffix(".mangadex.network")
            .is_some_and(|prefix| !prefix.is_empty() && !prefix.contains('.'));
    if url.scheme() != "https" || !allowed || url.port_or_known_default() != Some(443) {
        return Err(Error::new(
            "MangaDex@Home returned a server outside the approved MangaDex CDN",
        ));
    }
    Ok(())
}

fn cover_request(manga_id: &str, filename: &str, quality: &str) -> ImageRequest {
    let suffix = match quality {
        "original" => "",
        ".256.jpg" => ".256.jpg",
        _ => ".512.jpg",
    };
    ImageRequest::get(format!(
        "{UPLOADS_URL}/covers/{manga_id}/{filename}{suffix}"
    ))
    .header("User-Agent", NATIVE_USER_AGENT)
    .header("Referer", format!("{SITE_URL}/"))
    .header("Origin", SITE_URL)
}

fn image_headers() -> BTreeMap<String, String> {
    [
        ("User-Agent".to_owned(), NATIVE_USER_AGENT.to_owned()),
        ("Referer".to_owned(), format!("{SITE_URL}/")),
        ("Origin".to_owned(), SITE_URL.to_owned()),
    ]
    .into_iter()
    .collect()
}

fn ensure_allowed_rating(rating: Option<&str>) -> Result<()> {
    if rating_is_allowed(rating) {
        Ok(())
    } else {
        Err(Error::new(format!(
            "MangaDex title is unavailable in this build because its content rating is {}",
            rating.unwrap_or("missing")
        )))
    }
}

fn rating_is_allowed(rating: Option<&str>) -> bool {
    matches!(rating, Some("safe" | "suggestive"))
}

fn localized_title(manga: &MangaData, preferred: &str) -> String {
    exact_localized(&manga.attributes.title, preferred)
        .or_else(|| exact_alt_title(&manga.attributes.alt_titles, preferred))
        .or_else(|| exact_localized(&manga.attributes.title, "en"))
        .or_else(|| exact_alt_title(&manga.attributes.alt_titles, "en"))
        .or_else(|| {
            manga
                .attributes
                .original_language
                .as_deref()
                .and_then(|language| exact_localized(&manga.attributes.title, language))
        })
        .or_else(|| {
            manga
                .attributes
                .original_language
                .as_deref()
                .and_then(|language| exact_alt_title(&manga.attributes.alt_titles, language))
        })
        .or_else(|| {
            manga
                .attributes
                .title
                .values()
                .find(|value| !value.trim().is_empty())
                .cloned()
        })
        .or_else(|| {
            manga.attributes.alt_titles.iter().find_map(|titles| {
                titles
                    .values()
                    .find(|value| !value.trim().is_empty())
                    .cloned()
            })
        })
        .unwrap_or_else(|| format!("Untitled ({})", manga.id))
}

fn exact_localized(values: &BTreeMap<String, String>, language: &str) -> Option<String> {
    values
        .get(language)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn exact_alt_title(alt_titles: &[BTreeMap<String, String>], language: &str) -> Option<String> {
    alt_titles
        .iter()
        .find_map(|titles| exact_localized(titles, language))
}

fn localized_value(
    values: &BTreeMap<String, String>,
    preferred: &str,
    original: Option<&str>,
) -> Option<String> {
    [Some(preferred), Some("en"), original]
        .into_iter()
        .flatten()
        .find_map(|language| values.get(language))
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| {
            values
                .values()
                .find(|value| !value.trim().is_empty())
                .cloned()
        })
}

fn relationship_id<'a>(relationships: &'a [Relationship], kind: &str) -> Option<&'a str> {
    relationships
        .iter()
        .find(|relationship| relationship.kind == kind)
        .map(|relationship| relationship.id.as_str())
}

fn relationship_attribute<'a>(
    relationships: &'a [Relationship],
    kind: &str,
    attribute: &str,
) -> Option<&'a str> {
    relationships
        .iter()
        .find(|relationship| relationship.kind == kind)
        .and_then(|relationship| relationship.attributes.get(attribute))
        .and_then(Value::as_str)
}

fn relationship_names(relationships: &[Relationship], kind: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for relationship in relationships.iter().filter(|entry| entry.kind == kind) {
        if let Some(name) = relationship.attributes.get("name").and_then(Value::as_str) {
            if !name.trim().is_empty() {
                names.insert(name.trim().to_owned());
            }
        }
    }
    names.into_iter().collect()
}

fn translated_language() -> String {
    context::preference::<String>(LANGUAGE_KEY)
        .ok()
        .flatten()
        .filter(|value| language_codes().iter().any(|(_, code)| code == value))
        .unwrap_or_else(|| "en".to_owned())
}

fn cover_quality() -> String {
    context::preference::<String>(COVER_QUALITY_KEY)
        .ok()
        .flatten()
        .filter(|value| matches!(value.as_str(), "original" | ".512.jpg" | ".256.jpg"))
        .unwrap_or_else(|| ".512.jpg".to_owned())
}

fn data_saver() -> bool {
    context::preference::<bool>(DATA_SAVER_KEY)
        .ok()
        .flatten()
        .unwrap_or(false)
}

fn force_port_443() -> bool {
    context::preference::<bool>(FORCE_PORT_443_KEY)
        .ok()
        .flatten()
        .unwrap_or(true)
}

fn selected_original_languages(filters: &Value) -> Vec<String> {
    let from_filters = group_values(filters, "original_languages");
    let selected = if from_filters.is_empty() {
        context::preference::<Vec<String>>(ORIGINAL_LANGUAGES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        from_filters
    };
    selected
        .into_iter()
        .filter(|value| language_codes().iter().any(|(_, code)| code == value))
        .collect()
}

fn blocked_groups() -> Vec<String> {
    let configured = context::preference::<String>(BLOCKED_GROUPS_KEY)
        .ok()
        .flatten();
    let mut values = DEFAULT_BLOCKED_GROUPS
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    values.extend(uuid_list(configured.as_deref()));
    values.sort();
    values.dedup();
    values
}

fn blocked_uploaders() -> Vec<String> {
    let configured = context::preference::<String>(BLOCKED_UPLOADERS_KEY)
        .ok()
        .flatten();
    uuid_list(configured.as_deref())
}

fn uuid_list(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split([',', '\n', '\r', ' ', '\t'])
        .map(str::trim)
        .filter(|value| valid_uuid(value))
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn manga_id(candidate: &str) -> Result<String> {
    extract_uuid(candidate).ok_or_else(|| Error::new("Invalid MangaDex manga id or URL"))
}

fn chapter_id(candidate: &str) -> Result<String> {
    extract_uuid(candidate).ok_or_else(|| Error::new("Invalid MangaDex chapter id or URL"))
}

fn extract_uuid(candidate: &str) -> Option<String> {
    candidate
        .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == '-'))
        .find(|part| valid_uuid(part))
        .map(str::to_ascii_lowercase)
}

fn require_uuid<'a>(candidate: &'a str, label: &str) -> Result<&'a str> {
    if valid_uuid(candidate) {
        Ok(candidate)
    } else {
        Err(Error::new(format!("Invalid MangaDex {label} UUID")))
    }
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

fn supported_deep_link(candidate: &str) -> Result<Option<(&'static str, String)>> {
    let url = Url::parse(candidate).map_err(url_error)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https" || !matches!(host.as_str(), "mangadex.org" | "canary.mangadex.dev") {
        return Ok(None);
    }
    let segments = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    let kind = match segments.first().copied() {
        Some("title") => "title",
        Some("manga") => "manga",
        Some("chapter") => "chapter",
        _ => return Ok(None),
    };
    let Some(id) = segments.get(1).and_then(|segment| extract_uuid(segment)) else {
        return Ok(None);
    };
    Ok(Some((kind, id)))
}

fn parse_number(value: Option<&str>) -> Option<f32> {
    value?.trim().parse::<f32>().ok()
}

fn parse_date(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis())
}

fn aggregate_stats(response: &AggregateResponse) -> (u64, u64) {
    let volumes = response.volumes.len() as u64;
    let chapters = response
        .volumes
        .values()
        .map(|volume| volume.chapters.len() as u64)
        .sum();
    (volumes, chapters)
}

fn filter_definitions() -> Vec<FilterDefinition> {
    vec![
        FilterDefinition::Header {
            name: "Safe and suggestive titles only. Adult ratings are always rejected.".to_owned(),
        },
        FilterDefinition::Select {
            id: "sort".to_owned(),
            name: "Sort by".to_owned(),
            options: vec![
                option("Best match", "relevance:desc"),
                option("Latest chapter", "latestUploadedChapter:desc"),
                option("Most followed", "followedCount:desc"),
                option("Recently added", "createdAt:desc"),
                option("Recently updated", "updatedAt:desc"),
                option("Title (A-Z)", "title:asc"),
                option("Year (newest)", "year:desc"),
                option("Highest rated", "rating:desc"),
            ],
            default_index: 0,
        },
        check_group(
            "statuses",
            "Publication status",
            &[
                ("Ongoing", "ongoing"),
                ("Completed", "completed"),
                ("Hiatus", "hiatus"),
                ("Cancelled", "cancelled"),
            ],
        ),
        check_group(
            "demographics",
            "Demographic",
            &[
                ("Shounen", "shounen"),
                ("Shoujo", "shoujo"),
                ("Josei", "josei"),
                ("Seinen", "seinen"),
                ("None", "none"),
            ],
        ),
        check_group(
            "original_languages",
            "Original language",
            &language_codes()
                .iter()
                .map(|(label, code)| (*label, *code))
                .collect::<Vec<_>>(),
        ),
        FilterDefinition::Text {
            id: "included_tags".to_owned(),
            name: "Included tag UUIDs (comma-separated)".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Select {
            id: "included_tags_mode".to_owned(),
            name: "Included tags mode".to_owned(),
            options: vec![option("All (AND)", "AND"), option("Any (OR)", "OR")],
            default_index: 0,
        },
        FilterDefinition::Text {
            id: "excluded_tags".to_owned(),
            name: "Excluded tag UUIDs (comma-separated)".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Select {
            id: "excluded_tags_mode".to_owned(),
            name: "Excluded tags mode".to_owned(),
            options: vec![option("Any (OR)", "OR"), option("All (AND)", "AND")],
            default_index: 0,
        },
        FilterDefinition::Text {
            id: "year".to_owned(),
            name: "Publication year".to_owned(),
            default: String::new(),
        },
    ]
}

fn preference_definitions() -> Vec<PreferenceDefinition> {
    vec![
        PreferenceDefinition::Select {
            key: LANGUAGE_KEY.to_owned(),
            title: "Chapter language".to_owned(),
            options: language_codes()
                .iter()
                .map(|(label, code)| option(label, code))
                .collect(),
            default: "en".to_owned(),
        },
        PreferenceDefinition::MultiSelect {
            key: ORIGINAL_LANGUAGES_KEY.to_owned(),
            title: "Original languages".to_owned(),
            summary: Some("Leave empty to include every original language.".to_owned()),
            options: language_codes()
                .iter()
                .map(|(label, code)| option(label, code))
                .collect(),
            default: Vec::new(),
        },
        PreferenceDefinition::Select {
            key: COVER_QUALITY_KEY.to_owned(),
            title: "Cover quality".to_owned(),
            options: vec![
                option("Medium (512px)", ".512.jpg"),
                option("Low (256px)", ".256.jpg"),
                option("Original", "original"),
            ],
            default: ".512.jpg".to_owned(),
        },
        PreferenceDefinition::Switch {
            key: DATA_SAVER_KEY.to_owned(),
            title: "Use data-saver pages".to_owned(),
            summary: Some("Loads MangaDex's smaller page images.".to_owned()),
            default: false,
        },
        PreferenceDefinition::Switch {
            key: FORCE_PORT_443_KEY.to_owned(),
            title: "Force HTTPS port 443".to_owned(),
            summary: Some("Recommended for restrictive networks.".to_owned()),
            default: true,
        },
        PreferenceDefinition::Text {
            key: BLOCKED_GROUPS_KEY.to_owned(),
            title: "Additional blocked group UUIDs".to_owned(),
            summary: Some(
                "Comma-separated. MangaDex's official-link groups remain blocked by default."
                    .to_owned(),
            ),
            default: DEFAULT_BLOCKED_GROUPS.join(","),
            hint: Some("uuid, uuid".to_owned()),
            secure: false,
            multiline: true,
        },
        PreferenceDefinition::Text {
            key: BLOCKED_UPLOADERS_KEY.to_owned(),
            title: "Blocked uploader UUIDs".to_owned(),
            summary: Some("Comma-separated uploader UUIDs excluded from feeds.".to_owned()),
            default: String::new(),
            hint: Some("uuid, uuid".to_owned()),
            secure: false,
            multiline: true,
        },
    ]
}

fn language_codes() -> &'static [(&'static str, &'static str)] {
    &[
        ("English", "en"),
        ("Arabic", "ar"),
        ("Chinese (Simplified)", "zh"),
        ("Chinese (Traditional)", "zh-hk"),
        ("French", "fr"),
        ("German", "de"),
        ("Indonesian", "id"),
        ("Italian", "it"),
        ("Japanese", "ja"),
        ("Korean", "ko"),
        ("Polish", "pl"),
        ("Portuguese", "pt"),
        ("Portuguese (Brazil)", "pt-br"),
        ("Russian", "ru"),
        ("Spanish", "es"),
        ("Spanish (Latin America)", "es-la"),
        ("Thai", "th"),
        ("Turkish", "tr"),
        ("Ukrainian", "uk"),
        ("Vietnamese", "vi"),
    ]
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

fn check_group(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Group {
        id: id.to_owned(),
        name: name.to_owned(),
        filters: values
            .iter()
            .map(|(label, value)| FilterDefinition::CheckBox {
                id: (*value).to_owned(),
                name: (*label).to_owned(),
                default: false,
            })
            .collect(),
    }
}

fn text_value(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn select_value<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn group_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Object(entries)) => entries
            .iter()
            .filter(|(_, selected)| selected.as_bool().unwrap_or(false))
            .map(|(entry, _)| entry.clone())
            .collect(),
        Some(Value::Array(entries)) => entries
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(format!("MangaDex URL error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_sdk::manifest::Manifest;
    use serde_json::json;

    const MANIFEST: &str = include_str!("../manifest.json");
    const CATALOG: &str = include_str!("../fixtures/catalog.json");
    const DETAIL_SAFE: &str = include_str!("../fixtures/detail-safe.json");
    const DETAIL_ADULT: &str = include_str!("../fixtures/detail-adult.json");
    const CHAPTERS_PAGE_1: &str = include_str!("../fixtures/chapters-page-1.json");
    const CHAPTERS_PAGE_2: &str = include_str!("../fixtures/chapters-page-2.json");
    const AGGREGATE: &str = include_str!("../fixtures/aggregate.json");
    const AT_HOME_INITIAL: &str = include_str!("../fixtures/at-home-initial.json");
    const AT_HOME_REFRESHED: &str = include_str!("../fixtures/at-home-refreshed.json");

    #[test]
    fn manifest_is_play_safe_and_structurally_valid() {
        let manifest: Manifest = serde_json::from_str(MANIFEST).expect("manifest parses");
        manifest.validate().expect("manifest validates");
        assert_eq!(manifest.id, "mangadex");
        assert_eq!(manifest.publisher.id, "org.manatan.community.extensions");
        assert_eq!(
            manifest.sources[0].content_rating,
            manatan_sdk::manifest::ContentRating::Suggestive
        );
        assert!(!manifest.permissions.webview);
        assert!(!manifest.permissions.javascript);
        assert!(!manifest.permissions.assets);
        assert!(manifest.assets.is_empty());
        assert!(manifest
            .permissions
            .network
            .allow
            .contains(&"https://*.mangadex.network".to_owned()));
    }

    #[test]
    fn catalog_drops_adult_entries_and_prefers_requested_alt_title() {
        let response: CollectionResponse<MangaData> =
            serde_json::from_str(CATALOG).expect("catalog fixture parses");
        let parsed = parse_catalog(response).expect("catalog parses");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].title, "Journey of the Stars");
        assert_eq!(parsed.entries[0].content_rating.as_deref(), Some("safe"));
        let cover = parsed.entries[0].cover.as_ref().expect("cover");
        assert_eq!(
            cover.url,
            "https://uploads.mangadex.org/covers/11111111-1111-4111-8111-111111111111/safe-cover.jpg.512.jpg"
        );
        assert_eq!(
            cover.headers.get("User-Agent").map(String::as_str),
            Some(NATIVE_USER_AGENT)
        );
        assert!(parsed
            .entries
            .iter()
            .all(|item| item.content_rating.as_deref() != Some("pornographic")));
    }

    #[test]
    fn adult_and_unknown_details_fail_closed() {
        let adult: EntityResponse<MangaData> =
            serde_json::from_str(DETAIL_ADULT).expect("adult detail parses");
        let error = adult
            .data
            .to_catalog_item("en", ".512.jpg")
            .expect_err("adult detail is rejected");
        assert!(error.message.contains("erotica"));

        let mut unknown = adult.data;
        unknown.attributes.content_rating = None;
        assert!(unknown.to_catalog_item("en", ".512.jpg").is_err());
    }

    #[test]
    fn detail_parses_authors_artists_tags_and_metadata() {
        let response: EntityResponse<MangaData> =
            serde_json::from_str(DETAIL_SAFE).expect("safe detail parses");
        let item = response
            .data
            .to_catalog_item("en", "original")
            .expect("safe detail converts");
        assert_eq!(item.title, "Journey of the Stars");
        assert_eq!(item.authors, vec!["Fixture Author"]);
        assert_eq!(item.artists, vec!["Fixture Artist"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.extra["lastVolume"], "2");
        assert!(item.cover.expect("cover").url.ends_with("safe-cover.jpg"));
    }

    #[test]
    fn every_catalog_request_hard_codes_only_allowed_ratings() {
        let hostile_filters = json!({
            "contentRating": ["pornographic", "erotica"],
            "statuses": { "ongoing": true },
            "sort": "rating:desc"
        });
        let url = manga_list_url(3, None, "fixture", &hostile_filters).expect("url builds");
        let pairs = Url::parse(&url)
            .expect("url parses")
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        let ratings = pairs
            .iter()
            .filter(|(key, _)| key == "contentRating[]")
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ratings, vec!["safe", "suggestive"]);
        assert!(!url.contains("pornographic"));
        assert!(!url.contains("erotica"));
        assert!(pairs.contains(&("offset".to_owned(), "40".to_owned())));
    }

    #[test]
    fn latest_and_feed_requests_apply_safety_and_block_lists() {
        let latest = latest_feed_url(2).expect("latest URL");
        let feed = chapter_feed_url("11111111-1111-4111-8111-111111111111", 500).expect("feed URL");
        for url in [latest, feed] {
            let pairs = Url::parse(&url)
                .expect("url parses")
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>();
            assert!(pairs.contains(&("contentRating[]".to_owned(), "safe".to_owned())));
            assert!(pairs.contains(&("contentRating[]".to_owned(), "suggestive".to_owned())));
            assert!(pairs.iter().any(|(key, _)| key == "excludedGroups[]"));
            assert!(!pairs
                .iter()
                .any(|(_, value)| { matches!(value.as_str(), "erotica" | "pornographic") }));
        }
    }

    #[test]
    fn chapter_pages_parse_across_feed_pagination() {
        let first: CollectionResponse<ChapterData> =
            serde_json::from_str(CHAPTERS_PAGE_1).expect("page 1 parses");
        let second: CollectionResponse<ChapterData> =
            serde_json::from_str(CHAPTERS_PAGE_2).expect("page 2 parses");
        assert_eq!(first.offset + first.limit, 1);
        assert!(first.offset + first.limit < first.total);
        assert_eq!(second.offset + second.limit, second.total);

        let chapter = first.data[0].to_manga_chapter(0).expect("chapter converts");
        assert_eq!(chapter.chapter_number, Some(12.5));
        assert_eq!(chapter.volume_number, Some(2.0));
        assert_eq!(chapter.page_count, Some(2));
        assert_eq!(chapter.scanlators, vec!["Fixture Group"]);
        assert!(chapter.date_uploaded.is_some());
    }

    #[test]
    fn aggregate_fixture_reports_volume_and_chapter_counts() {
        let aggregate: AggregateResponse =
            serde_json::from_str(AGGREGATE).expect("aggregate parses");
        assert_eq!(aggregate_stats(&aggregate), (1, 2));
    }

    #[test]
    fn lazy_pages_refresh_expiring_at_home_urls() {
        let initial: AtHomeResponse =
            serde_json::from_str(AT_HOME_INITIAL).expect("initial at-home parses");
        let refreshed: AtHomeResponse =
            serde_json::from_str(AT_HOME_REFRESHED).expect("refreshed at-home parses");
        let pages = pages_from_at_home("66666666-6666-4666-8666-666666666666", &initial, false)
            .expect("pages parse");
        assert_eq!(pages.len(), 2);
        let initial_url = match &pages[0].content {
            PageContent::Lazy { url: Some(url), .. } => url,
            other => panic!("expected lazy URL, got {other:?}"),
        };
        assert_eq!(
            initial_url,
            "https://s1.mangadex.network/data/initial-hash/page-1.jpg"
        );
        assert_eq!(
            at_home_image_url(&refreshed, 0, false).expect("refresh resolves"),
            "https://s9.mangadex.network/data/refreshed-hash/new-page-1.jpg"
        );
        assert_eq!(
            at_home_image_url(&refreshed, 1, true).expect("saver resolves"),
            "https://s9.mangadex.network/data-saver/refreshed-hash/new-page-2-saver.jpg"
        );
    }

    #[test]
    fn at_home_rejects_unapproved_or_ambiguous_hosts() {
        assert!(validate_at_home_base("https://s1.mangadex.network").is_ok());
        assert!(validate_at_home_base("https://uploads.mangadex.org").is_ok());
        assert!(validate_at_home_base("https://mangadex.network.attacker.invalid").is_err());
        assert!(validate_at_home_base("http://s1.mangadex.network").is_err());
        assert!(validate_at_home_base("https://nested.s1.mangadex.network").is_err());
    }

    #[test]
    fn deep_link_parser_accepts_only_owned_https_hosts() {
        assert_eq!(
            supported_deep_link(
                "https://mangadex.org/title/11111111-1111-4111-8111-111111111111/slug"
            )
            .expect("link parses"),
            Some(("title", "11111111-1111-4111-8111-111111111111".to_owned()))
        );
        assert!(supported_deep_link(
            "https://mangadex.org.attacker.invalid/title/11111111-1111-4111-8111-111111111111"
        )
        .expect("link parses")
        .is_none());
        assert!(supported_deep_link(
            "http://mangadex.org/title/11111111-1111-4111-8111-111111111111"
        )
        .expect("link parses")
        .is_none());
    }
}
