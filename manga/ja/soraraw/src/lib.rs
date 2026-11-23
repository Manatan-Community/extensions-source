use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose, Engine};
use chrono::DateTime;
use manatan_sdk::{
    client::{Client, BROWSER_USER_AGENT},
    context, CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage,
    MangaSource, OptionItem, PageContent, Paged, PreferenceDefinition, Result, UrlResolveResult,
};
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const BASE_URL: &str = "https://soraraw.com";
const COVER_BASE_URL: &str = "https://i.mangaraw.lat";
const IMAGE_API_URL: &str = "https://api.mangarawgo.site";
const IMAGE_DOMAIN: &str = "rawcontent.top";
const IMAGE_MANIFEST_KEY: &[u8] = b"/fuCkYou!!!";
const LANGUAGE: &str = "ja";
const DATASET_BATCH_SIZE: u32 = 6;
const MAX_DATASET_PAGES: u32 = 52;
const MAX_DATASET_PAGE_BYTES: u64 = 4 * 1024 * 1024;

pub struct SoraRawSource {
    client: Client,
    dataset: Option<Vec<DatasetManga>>,
}

impl Default for SoraRawSource {
    fn default() -> Self {
        Self {
            client: Client::browser().header("Referer", format!("{BASE_URL}/")),
            dataset: None,
        }
    }
}

impl SoraRawSource {
    fn text(&self, url: &str, max_body_bytes: u64) -> Result<String> {
        self.client
            .get(url)
            .max_body_bytes(max_body_bytes)
            .send()?
            .error_for_status()?
            .text()
            .map(ToOwned::to_owned)
    }

    fn next_data(&self, url: &str) -> Result<Value> {
        parse_next_data(&self.text(url, 3_000_000)?)
    }

    fn show_adult() -> bool {
        context::preference::<bool>("show_adult")
            .ok()
            .flatten()
            .unwrap_or(true)
    }

    fn json(&self, path: &str) -> Result<Value> {
        serde_json::from_str(&self.text(&format!("{BASE_URL}/{path}"), 2_000_000)?)
            .map_err(Into::into)
    }

    fn listing_page(&self, path: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        if path.starts_with("top/") {
            return parse_ranking(&self.json(path)?, Self::show_adult());
        }
        let url = if page == 1 {
            format!("{BASE_URL}/{path}")
        } else {
            format!("{BASE_URL}/{path}/page/{page}")
        };
        parse_listing(&self.next_data(&url)?, page)
    }

    fn genres(&self, show_adult: bool) -> Result<Vec<Genre>> {
        let genres = serde_json::from_value(self.json("genres.json")?)?;
        Ok(group_genres(genres, show_adult))
    }

    fn dataset(&mut self) -> Result<&[DatasetManga]> {
        if self.dataset.is_none() {
            let pages = self.json("config.json")?["m"]
                .as_u64()
                .filter(|count| (1..=u64::from(MAX_DATASET_PAGES)).contains(count))
                .ok_or_else(|| Error::new("SoraRaw returned an invalid catalog page count"))?
                as u32;
            let mut entries = Vec::new();
            for first in (1..=pages).step_by(DATASET_BATCH_SIZE as usize) {
                let requests = (first..=(first + DATASET_BATCH_SIZE - 1).min(pages))
                    .map(|page| {
                        self.client
                            .get(format!("{BASE_URL}/mangas_{page}.json"))
                            .max_body_bytes(MAX_DATASET_PAGE_BYTES)
                    })
                    .collect();
                for response in Client::send_many(requests, DATASET_BATCH_SIZE as u16) {
                    let page: DatasetPage = response?.error_for_status()?.json()?;
                    entries.extend(page.list);
                }
            }
            self.dataset = Some(entries);
        }
        Ok(self.dataset.as_deref().unwrap_or_default())
    }

    fn search_catalog(
        &mut self,
        query: &str,
        page: u32,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        let show_adult = Self::show_adult();
        let genre = match selected(filters, "genre") {
            Some(slug) => self
                .genres(show_adult)?
                .into_iter()
                .find(|genre| genre.slug == slug),
            None => None,
        };
        if query.is_empty() {
            if let Some(genre) = genre {
                return self.listing_page(&format!("genre/{}", genre.slug), page);
            }
            let path = LISTS
                .iter()
                .find(|(_, path)| Some(*path) == selected(filters, "list"))
                .unwrap_or(&LISTS[0])
                .1;
            return self.listing_page(path, page);
        }
        if page > 1 {
            return Ok(Paged::new(Vec::new(), false));
        }
        Ok(search_entries(
            self.dataset()?,
            query,
            genre.as_ref(),
            show_adult,
        ))
    }
}

impl MangaSource for SoraRawSource {
    fn refresh(
        &mut self,
        request: manatan_sdk::CatalogRefreshRequest<MangaChapter>,
    ) -> Result<Option<manatan_sdk::CatalogRefresh<MangaChapter>>> {
        if !request.fetch_details && !request.fetch_entries {
            return Ok(Some(manatan_sdk::CatalogRefresh::default()));
        }
        let slug = item_slug(&request.item)?;
        let data = self.next_data(&manga_url(&slug))?;
        Ok(Some(manatan_sdk::CatalogRefresh {
            item: request
                .fetch_details
                .then(|| parse_details(&data, &slug))
                .transpose()?,
            entries: request
                .fetch_entries
                .then(|| parse_chapters(&data, &slug))
                .transpose()?,
        }))
    }
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page(LISTS[0].1, page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page("newest", page)
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
            _ => Err(Error::new("unknown SoraRaw listing")),
        }
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if query.starts_with("https://") || query.starts_with("http://") {
            if let Some(resolved) = self.handle_url(query)? {
                return Ok(Paged::new(resolved.item.into_iter().collect(), false));
            }
        }
        self.search_catalog(query, page, filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let slug = item_slug(&item)?;
        let data = self.next_data(&manga_url(&slug))?;
        parse_details(&data, &slug)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let slug = item_slug(&item)?;
        let data = self.next_data(&manga_url(&slug))?;
        parse_chapters(&data, &slug)
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let slug = item_slug(&item)?;
        let chapter_url =
            canonical_chapter_url(&slug, chapter.url.as_deref().unwrap_or(&chapter.key))
                .ok_or_else(|| Error::new("SoraRaw chapter has no chapter key"))?;
        let data = self.next_data(&chapter_url)?;
        let chapter_id = data
            .pointer("/props/pageProps/data/chapter/id")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("SoraRaw chapter has no id"))?;
        let manga_id = data
            .pointer("/props/pageProps/data/chapter/manga/id")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("SoraRaw chapter has no manga id"))?;
        let response = self
            .client
            .get(format!("{IMAGE_API_URL}/{manga_id}/{chapter_id}.json"))
            .max_body_bytes(1_000_000)
            .send()?
            .error_for_status()?;
        parse_pages(response.bytes(), chapter_id, &chapter_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filter_definitions(
            self.genres(Self::show_adult()).unwrap_or_default(),
        ))
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(vec![PreferenceDefinition::Switch {
            key: "show_adult".into(),
            title: "Show 18+ titles".into(),
            default: true,
            summary: Some(
                "Includes 18+ titles in search and switches rankings to the 18+ list.".into(),
            ),
        }])
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(manga_url(&item_slug(item)?)))
    }

    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let slug = item_slug(item)?;
        Ok(canonical_chapter_url(
            &slug,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        ))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let parsed = Url::parse(candidate).map_err(url_error)?;
        if !matches!(parsed.host_str(), Some("soraraw.com" | "www.soraraw.com")) {
            return Ok(None);
        }
        let Some(slug) = manga_slug(parsed.path()) else {
            return Ok(None);
        };
        let item = CatalogItem {
            key: slug.clone(),
            title: slug.clone(),
            url: Some(manga_url(&slug)),
            language: Some(LANGUAGE.to_owned()),
            content_rating: Some("adult".to_owned()),
            viewer: Some(json!("rtl")),
            ..CatalogItem::default()
        };
        let mut result = UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        };
        if let Some(key) = chapter_key(parsed.path(), &slug) {
            result.chapter_key = Some(key.clone());
            result.manga_chapter = Some(MangaChapter {
                key: key.clone(),
                chapter_number: chapter_number(&key),
                language: Some(LANGUAGE.to_owned()),
                url: Some(format!("{BASE_URL}/manga/{slug}/{key}")),
                ..MangaChapter::default()
            });
        }
        Ok(Some(result))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("soraraw", SoraRawSource::default())
);

#[derive(Debug, Deserialize)]
struct DatasetPage {
    #[serde(default)]
    list: Vec<DatasetManga>,
}

#[derive(Clone, Debug, Deserialize)]
struct DatasetManga {
    id: u64,
    name: String,
    #[serde(default)]
    alt_names: Option<String>,
    slug: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default, rename = "img")]
    image: Option<String>,
    #[serde(default)]
    genres: Vec<u64>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    is_adult: Option<String>,
    #[serde(default)]
    views: u64,
}

impl DatasetManga {
    fn into_catalog(self) -> CatalogItem {
        catalog_item(&json!({
            "id": self.id,
            "name": self.name,
            "slug": self.slug,
            "author": self.author,
            "image": self.image,
            "mode": self.mode,
            "type": self.kind,
            "is_adult": self.is_adult,
        }))
        .unwrap_or_else(|_| CatalogItem::new(self.slug, self.name))
    }
}

#[derive(Debug, Deserialize)]
struct EncryptedManifest {
    d: String,
}

#[derive(Debug, Deserialize)]
struct PageRecord {
    id: u64,
    order: u32,
}

fn parse_next_data(html: &str) -> Result<Value> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("script#__NEXT_DATA__")
        .map_err(|error| Error::new(format!("invalid SoraRaw selector: {error}")))?;
    let raw = document
        .select(&selector)
        .next()
        .map(|element| element.inner_html())
        .ok_or_else(|| Error::new("SoraRaw page has no __NEXT_DATA__"))?;
    serde_json::from_str(&raw).map_err(Into::into)
}

fn parse_listing(data: &Value, current_page: u32) -> Result<Paged<CatalogItem>> {
    let page_data = data
        .pointer("/props/pageProps/data")
        .ok_or_else(|| Error::new("SoraRaw listing has no page data"))?;
    let values = page_data
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("SoraRaw listing has no results"))?;
    let entries = values
        .iter()
        .map(catalog_item)
        .collect::<Result<Vec<_>>>()?;
    let total_pages = page_data
        .pointer("/pagination/total_page")
        .and_then(Value::as_u64)
        .unwrap_or(current_page as u64);
    Ok(Paged::new(entries, (current_page as u64) < total_pages))
}

fn parse_ranking(value: &Value, show_adult: bool) -> Result<Paged<CatalogItem>> {
    let key = if show_adult { "manga18s" } else { "mangas" };
    let entries = value[key]
        .as_array()
        .ok_or_else(|| Error::new(format!("SoraRaw ranking has no {key} list")))?
        .iter()
        .map(catalog_item)
        .collect::<Result<Vec<_>>>()?;
    Ok(Paged::new(entries, false))
}

fn search_entries(
    dataset: &[DatasetManga],
    query: &str,
    genre: Option<&Genre>,
    show_adult: bool,
) -> Paged<CatalogItem> {
    let query = query.to_lowercase();
    let mut matches: Vec<_> = dataset
        .iter()
        .filter(|entry| {
            let text = format!(
                "{} {} {}",
                entry.name,
                entry.alt_names.as_deref().unwrap_or_default(),
                entry.author.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            query.split_whitespace().all(|word| text.contains(word))
                && genre.is_none_or(|genre| entry.genres.iter().any(|id| genre.ids.contains(id)))
                && (show_adult || entry.is_adult.as_deref() != Some("yes"))
        })
        .collect();
    matches.sort_by_key(|entry| std::cmp::Reverse(entry.views));
    Paged::new(
        matches
            .into_iter()
            .cloned()
            .map(DatasetManga::into_catalog)
            .collect(),
        false,
    )
}

fn catalog_item(value: &Value) -> Result<CatalogItem> {
    let slug =
        string(value, "slug").ok_or_else(|| Error::new("SoraRaw catalog item has no slug"))?;
    let title = string(value, "name")
        .or_else(|| string(value, "title_seo"))
        .ok_or_else(|| Error::new("SoraRaw catalog item has no title"))?;
    let image = string(value, "thumbnail")
        .or_else(|| string(value, "image"))
        .or_else(|| string(value, "img"))
        .filter(|image| image != "null")
        .map(|image| cover_url(&image))
        .transpose()?;
    let author = string(value, "author").filter(|author| !author.is_empty());
    let tags = value
        .get("genres")
        .and_then(Value::as_array)
        .map(|genres| {
            genres
                .iter()
                .filter_map(|genre| genre.get("name").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(CatalogItem {
        key: slug.clone(),
        title,
        url: Some(manga_url(&slug)),
        cover: image.map(image_request),
        authors: author.into_iter().collect(),
        tags,
        status: string(value, "type").map(|kind| json!(status(&kind))),
        language: Some(LANGUAGE.to_owned()),
        rating: number(value.get("rate")),
        content_rating: Some(content_rating(value).to_owned()),
        viewer: Some(viewer(value.get("mode").and_then(Value::as_str))),
        ..CatalogItem::default()
    })
}

fn parse_details(data: &Value, slug: &str) -> Result<CatalogItem> {
    let manga = data
        .pointer("/props/pageProps/data/manga")
        .ok_or_else(|| Error::new("SoraRaw details page has no manga"))?;
    let mut item = catalog_item(manga)?;
    item.key = slug.to_owned();
    item.url = Some(manga_url(slug));
    item.description = description(manga);
    item.initialized = true;
    Ok(item)
}

fn parse_chapters(data: &Value, slug: &str) -> Result<Vec<MangaChapter>> {
    let chapters = data
        .pointer("/props/pageProps/data/manga/chapters")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("SoraRaw details page has no chapters"))?;
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for value in chapters {
        let key = string(value, "path")
            .and_then(|path| {
                path.strip_prefix(&format!("{slug}-"))
                    .map(ToOwned::to_owned)
            })
            .or_else(|| {
                number(value.get("order")).map(|order| format!("ch-{}", display_number(order)))
            })
            .ok_or_else(|| Error::new("SoraRaw chapter has no key"))?;
        if !key.starts_with("ch-") || !seen.insert(key.clone()) {
            continue;
        }
        let title = string(value, "title").filter(|title| !title.is_empty());
        parsed.push(MangaChapter {
            key: key.clone(),
            title,
            chapter_number: number(value.get("order")).or_else(|| chapter_number(&key)),
            date_uploaded: string(value, "published_at")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
                .map(|date| date.timestamp_millis()),
            language: Some(LANGUAGE.to_owned()),
            url: Some(format!("{BASE_URL}/manga/{slug}/{key}")),
            ..MangaChapter::default()
        });
    }
    if parsed.is_empty() {
        return Err(Error::new("SoraRaw details page has no readable chapters"));
    }
    // SoraRaw has returned both scrambled and newest-first chapter arrays.
    // Normalize its output to the SDK's newest-first contract.
    parsed.sort_by(|left, right| {
        right
            .chapter_number
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&left.chapter_number.unwrap_or(f32::NEG_INFINITY))
            .then_with(|| right.date_uploaded.cmp(&left.date_uploaded))
            .then_with(|| right.key.cmp(&left.key))
    });
    Ok(parsed)
}

fn parse_pages(bytes: &[u8], chapter_id: u64, chapter_url: &str) -> Result<Vec<MangaPage>> {
    let manifest: EncryptedManifest = serde_json::from_slice(bytes)?;
    let encrypted = general_purpose::URL_SAFE_NO_PAD
        .decode(&manifest.d)
        .or_else(|_| general_purpose::URL_SAFE.decode(&manifest.d))
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(&manifest.d))
        .or_else(|_| general_purpose::STANDARD.decode(&manifest.d))
        .map_err(|error| Error::new(format!("SoraRaw page manifest is not base64: {error}")))?;
    let decrypted = encrypted
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ IMAGE_MANIFEST_KEY[index % IMAGE_MANIFEST_KEY.len()])
        .collect::<Vec<_>>();
    let mut records: Vec<PageRecord> = serde_json::from_slice(&decrypted)?;
    records.sort_by_key(|record| record.order);
    records.dedup_by_key(|record| record.order);
    if records.is_empty() {
        return Err(Error::new("SoraRaw chapter image manifest is empty"));
    }
    let server = chapter_id % 4 + 1;
    let mut context = BTreeMap::new();
    context.insert("Referer".to_owned(), chapter_url.to_owned());
    context.insert("User-Agent".to_owned(), BROWSER_USER_AGENT.to_owned());
    Ok(records
        .into_iter()
        .map(|record| MangaPage {
            content: PageContent::Url {
                url: format!(
                    "https://lh{server}.{IMAGE_DOMAIN}/c{chapter_id}/{:03}_{}.webp",
                    record.order, record.id
                ),
                context: Some(context.clone()),
            },
            description: Some(format!("Page {}", record.order)),
            ..MangaPage::default()
        })
        .collect())
}

fn description(manga: &Value) -> Option<String> {
    if let Some(value) = string(manga, "description").filter(|value| !value.is_empty()) {
        return Some(clean_html(&value));
    }
    let content = string(manga, "content")?;
    let editor: Value = serde_json::from_str(&content).ok()?;
    let paragraphs = editor
        .get("blocks")?
        .as_array()?
        .iter()
        .filter_map(|block| block.pointer("/data/text").and_then(Value::as_str))
        .map(clean_html)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"))
}

fn clean_html(value: &str) -> String {
    let fragment = Html::parse_fragment(value);
    clean_text(&fragment.root_element().text().collect::<Vec<_>>().join(" "))
}

const LISTS: &[(&str, &str)] = &[
    ("Weekly popular", "top/last7Days.json"),
    ("Monthly popular", "top/last30Days.json"),
    ("Trending", "top/rising.json"),
    ("All time", "top/lifetime.json"),
    ("Latest", "newest"),
    ("Ongoing", "on-going"),
    ("Completed", "complete"),
];

#[derive(Deserialize)]
struct GenreEntry {
    id: u64,
    name: String,
    slug: String,
    total: u64,
    is_adult: String,
}

struct Genre {
    name: String,
    slug: String,
    ids: BTreeSet<u64>,
    total: u64,
    largest: u64,
}

fn group_genres(entries: Vec<GenreEntry>, show_adult: bool) -> Vec<Genre> {
    let mut grouped = BTreeMap::new();
    for entry in entries {
        if entry.slug.is_empty() || (!show_adult && entry.is_adult == "yes") {
            continue;
        }
        let genre = grouped.entry(entry.slug.clone()).or_insert_with(|| Genre {
            name: entry.name.clone(),
            slug: entry.slug,
            ids: BTreeSet::new(),
            total: 0,
            largest: 0,
        });
        genre.ids.insert(entry.id);
        genre.total += entry.total;
        if entry.total > genre.largest {
            genre.name = entry.name;
            genre.largest = entry.total;
        }
    }
    let mut genres: Vec<_> = grouped.into_values().collect();
    genres.sort_by_key(|genre| std::cmp::Reverse(genre.total));
    genres
}

fn filter_definitions(genres: Vec<Genre>) -> Vec<FilterDefinition> {
    let mut filters = vec![FilterDefinition::Select {
        id: "list".into(),
        name: "List".into(),
        default_index: 0,
        options: LISTS
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
    }];
    if !genres.is_empty() {
        let options = std::iter::once(OptionItem {
            label: "All".into(),
            value: "".into(),
        })
        .chain(genres.into_iter().map(|genre| OptionItem {
            label: format!("{} ({})", genre.name, genre.total),
            value: genre.slug,
        }))
        .collect();
        filters.push(FilterDefinition::Select {
            id: "genre".into(),
            name: "Genre".into(),
            default_index: 0,
            options,
        });
    }
    filters
}

fn selected<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn number(value: Option<&Value>) -> Option<f32> {
    match value? {
        Value::Number(value) => value.as_f64().map(|value| value as f32),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn status(kind: &str) -> &'static str {
    match kind {
        "complete" => "completed",
        "incomplete" => "ongoing",
        _ => "unknown",
    }
}

fn content_rating(value: &Value) -> &'static str {
    if value.get("is_adult").and_then(Value::as_str) == Some("yes") {
        "adult"
    } else {
        "safe"
    }
}

fn viewer(mode: Option<&str>) -> Value {
    if mode == Some("vertical") {
        json!("vertical")
    } else {
        json!("rtl")
    }
}

fn cover_url(candidate: &str) -> Result<String> {
    if candidate.starts_with("https://") || candidate.starts_with("http://") {
        return Ok(candidate.to_owned());
    }
    Ok(format!(
        "{COVER_BASE_URL}/{}",
        candidate.trim_start_matches('/')
    ))
}

fn image_request(url: String) -> ImageRequest {
    ImageRequest::get(url)
        .header("Referer", format!("{BASE_URL}/"))
        .header("User-Agent", BROWSER_USER_AGENT)
}

fn item_slug(item: &CatalogItem) -> Result<String> {
    manga_slug(item.url.as_deref().unwrap_or(&item.key))
        .or_else(|| (!item.key.is_empty()).then(|| item.key.clone()))
        .ok_or_else(|| Error::new("SoraRaw item has no manga slug"))
}

fn manga_url(slug: &str) -> String {
    format!("{BASE_URL}/manga/{slug}")
}

fn manga_slug(candidate: &str) -> Option<String> {
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" {
        return None;
    }
    let slug = segments.next()?.trim();
    (!slug.is_empty()).then(|| slug.to_owned())
}

fn chapter_key(candidate: &str, slug: &str) -> Option<String> {
    let path = candidate_path(candidate);
    let mut segments = path.trim_matches('/').split('/');
    if segments.next()? != "manga" || segments.next()? != slug {
        return None;
    }
    let key = segments.next()?.trim();
    (key.starts_with("ch-") && segments.next().is_none()).then(|| key.to_owned())
}

fn canonical_chapter_url(slug: &str, candidate: &str) -> Option<String> {
    let key = if candidate.starts_with("ch-") {
        candidate.trim_matches('/').to_owned()
    } else {
        chapter_key(candidate, slug)?
    };
    Some(format!("{BASE_URL}/manga/{slug}/{key}"))
}

fn candidate_path(candidate: &str) -> &str {
    let path = candidate
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|index| &rest[index..]))
        .unwrap_or(candidate);
    path.split(['?', '#']).next().unwrap_or(path)
}

fn chapter_number(key: &str) -> Option<f32> {
    key.strip_prefix("ch-")?.replace('-', ".").parse().ok()
}

fn display_number(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as u64)
    } else {
        value.to_string().replace('.', "-")
    }
}

fn clean_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\u{a0}', " ")
}

fn url_error(error: impl ToString) -> Error {
    Error::new(format!("SoraRaw URL error: {}", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_without_requested_changes_preserves_host_state() {
        let mut source = SoraRawSource::default();
        let result = source
            .refresh(manatan_sdk::CatalogRefreshRequest {
                item: CatalogItem::default(),
                entries: Vec::new(),
                fetch_details: false,
                fetch_entries: false,
            })
            .unwrap()
            .unwrap();
        assert!(result.item.is_none());
        assert!(result.entries.is_none());
    }
    use sha2::{Digest, Sha256};

    const LISTING: &str = include_str!("../fixtures/listing.html");
    const DETAILS: &str = include_str!("../fixtures/details.html");
    const CHAPTER: &str = include_str!("../fixtures/chapter.html");
    const IMAGES: &[u8] = include_bytes!("../fixtures/images.json");
    const MANIFEST: &str = include_str!("../manifest.json");

    #[test]
    fn parses_listing_metadata_and_pagination() {
        let data = parse_next_data(LISTING).expect("next data parses");
        let page = parse_listing(&data, 1).expect("listing parses");
        assert_eq!(page.entries.len(), 2);
        assert!(page.has_next_page);
        assert_eq!(page.entries[0].key, "mokushiroku-no-yon-kishi-57357");
        assert_eq!(page.entries[0].authors, vec!["鈴木央"]);
        assert_eq!(page.entries[0].viewer, Some(json!("rtl")));
        assert_eq!(page.entries[1].viewer, Some(json!("vertical")));
        assert_eq!(page.entries[1].content_rating.as_deref(), Some("adult"));
    }

    #[test]
    fn parses_details_description_tags_and_status() {
        let data = parse_next_data(DETAILS).expect("next data parses");
        let item = parse_details(&data, "mokushiroku-no-yon-kishi-57357").expect("details parse");
        assert!(item.initialized);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.tags, vec!["アクション", "ファンタジー"]);
        assert!(item
            .description
            .as_deref()
            .is_some_and(|text| text.contains("少年は果て無き旅路")));
    }

    #[test]
    fn parses_newest_first_chapters_with_dates() {
        let data = parse_next_data(DETAILS).expect("next data parses");
        let chapters =
            parse_chapters(&data, "mokushiroku-no-yon-kishi-57357").expect("chapters parse");
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].key, "ch-247");
        assert_eq!(chapters[0].chapter_number, Some(247.0));
        assert!(chapters[0].date_uploaded.is_some());
    }

    #[test]
    fn sorts_scrambled_fractional_chapters_newest_first() {
        let data = json!({
            "props": { "pageProps": { "data": { "manga": { "chapters": [
                { "path": "example-ch-1", "order": 1, "published_at": "2026-01-01T00:00:00Z" },
                { "path": "example-ch-9", "order": 9, "published_at": "2026-03-01T00:00:00Z" },
                { "path": "example-ch-2-1", "order": 2.1, "published_at": "2026-02-01T00:00:00Z" },
                { "path": "example-ch-2", "order": 2, "published_at": "2026-01-15T00:00:00Z" }
            ] } } } }
        });

        let chapters = parse_chapters(&data, "example").expect("chapters parse");
        assert_eq!(
            chapters
                .iter()
                .map(|chapter| chapter.chapter_number)
                .collect::<Vec<_>>(),
            vec![Some(9.0), Some(2.1), Some(2.0), Some(1.0)]
        );
    }

    #[test]
    fn decrypts_page_manifest_and_builds_sharded_urls() {
        let pages = parse_pages(IMAGES, 785706, "https://soraraw.com/manga/example/ch-247")
            .expect("pages parse");
        assert_eq!(pages.len(), 2);
        let PageContent::Url { url, context } = &pages[1].content else {
            panic!("expected URL page");
        };
        assert_eq!(url, "https://lh3.rawcontent.top/c785706/002_503.webp");
        assert_eq!(
            context.as_ref().and_then(|headers| headers.get("Referer")),
            Some(&"https://soraraw.com/manga/example/ch-247".to_owned())
        );
    }

    #[test]
    fn accepts_standard_base64_page_manifests() {
        let plaintext = r#"[{"id":1,"order":1,"note":"日"}]"#;
        let encrypted = plaintext
            .bytes()
            .enumerate()
            .map(|(index, byte)| byte ^ IMAGE_MANIFEST_KEY[index % IMAGE_MANIFEST_KEY.len()])
            .collect::<Vec<_>>();
        let encoded = general_purpose::STANDARD_NO_PAD.encode(encrypted);
        assert!(encoded.contains('+') || encoded.contains('/'));
        let manifest = format!(r#"{{"d":"{encoded}"}}"#);

        let pages =
            parse_pages(manifest.as_bytes(), 2, BASE_URL).expect("standard Base64 manifest parses");
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn empty_or_invalid_page_manifests_fail_closed() {
        let empty = {
            let encrypted = b"[]"
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ IMAGE_MANIFEST_KEY[index % IMAGE_MANIFEST_KEY.len()])
                .collect::<Vec<_>>();
            format!(
                r#"{{"d":"{}"}}"#,
                general_purpose::URL_SAFE_NO_PAD.encode(encrypted)
            )
        };
        assert!(parse_pages(empty.as_bytes(), 1, BASE_URL).is_err());
        assert!(parse_pages(br#"{"d":"not base64!"}"#, 1, BASE_URL).is_err());
    }

    #[test]
    fn deep_links_resolve_to_canonical_item_and_chapter() {
        let mut source = SoraRawSource::default();
        let resolved = source
            .handle_url("https://soraraw.com/manga/mokushiroku-no-yon-kishi-57357/ch-74-1?x=1")
            .expect("URL parses")
            .expect("URL resolves");
        assert_eq!(
            resolved.item.expect("item").key,
            "mokushiroku-no-yon-kishi-57357"
        );
        assert_eq!(resolved.chapter_key.as_deref(), Some("ch-74-1"));
        assert_eq!(
            resolved
                .manga_chapter
                .and_then(|chapter| chapter.chapter_number),
            Some(74.1)
        );
    }

    #[test]
    fn matches_site_adult_ranking_and_keyword_search() {
        let ranking = json!({"manga18s":[{"slug":"adult","name":"Adult list"}],
            "mangas":[{"slug":"general","name":"General list"}]});
        let mut source = SoraRawSource::default();
        assert!(matches!(
            &source.preferences().unwrap()[0],
            PreferenceDefinition::Switch { default: true, .. }
        ));
        assert_eq!(
            parse_ranking(&ranking, true).unwrap().entries[0].key,
            "adult"
        );
        assert_eq!(
            parse_ranking(&ranking, false).unwrap().entries[0].key,
            "general"
        );
        assert!(!parse_ranking(&ranking, true).unwrap().has_next_page);
        let dataset = serde_json::from_value::<Vec<DatasetManga>>(json!([
            {"id":1,"name":"Alpha","alt_names":"Beta","slug":"alpha","is_adult":"yes","views":2},
            {"id":2,"name":"Beta","author":"Alpha","slug":"beta","is_adult":"no","views":1}
        ]))
        .unwrap();
        let keys = |adult| {
            search_entries(&dataset, "beta alpha", None, adult)
                .entries
                .into_iter()
                .map(|item| item.key)
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(true), ["alpha", "beta"]);
        assert_eq!(keys(false), ["beta"]);
    }

    #[test]
    fn fixture_chapter_contract_contains_required_ids() {
        let data = parse_next_data(CHAPTER).expect("next data parses");
        assert_eq!(
            data.pointer("/props/pageProps/data/chapter/id")
                .and_then(Value::as_u64),
            Some(785706)
        );
        assert_eq!(
            data.pointer("/props/pageProps/data/chapter/manga/id")
                .and_then(Value::as_u64),
            Some(57357)
        );
    }

    #[test]
    fn manifest_and_icon_contract_are_consistent() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");
        assert_eq!(manifest["id"], "soraraw");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(manifest["permissions"]["webview"], false);
        assert_eq!(manifest["permissions"]["javascript"], false);
        let icon = include_bytes!("../assets/icon.png");
        let digest = format!("{:x}", Sha256::digest(icon));
        assert_eq!(manifest["assets"][0]["sha256"], digest);
    }
}
