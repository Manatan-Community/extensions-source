use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose, Engine};
use chrono::DateTime;
use manatan_sdk::{
    client::{Client, BROWSER_USER_AGENT},
    CatalogItem, Error, FilterDefinition, ImageRequest, MangaChapter, MangaPage, MangaSource,
    OptionItem, PageContent, Paged, Result, UrlResolveResult,
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
const SEARCH_PAGE_SIZE: usize = 48;
const DATASET_BATCH_SIZE: u32 = 13;
const MAX_DATASET_PAGES: u32 = 52;

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

    fn listing_url(&self, kind: &str, page: u32) -> Result<String> {
        let page = page.max(1);
        match (kind, page) {
            ("popular", _) => Ok(format!("{BASE_URL}/search")),
            ("latest", 1) => Ok(format!("{BASE_URL}/newest")),
            ("latest", page) => Ok(format!("{BASE_URL}/newest/page/{page}")),
            _ => Err(Error::new(format!("unknown SoraRaw listing {kind:?}"))),
        }
    }

    fn listing_page(&self, kind: &str, page: u32) -> Result<Paged<CatalogItem>> {
        let page = page.max(1);
        let data = self.next_data(&self.listing_url(kind, page)?)?;
        if kind == "popular" {
            let values = data
                .pointer("/props/pageProps/data")
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("SoraRaw popular page has no catalog data"))?;
            return paginate_values(values, page);
        }
        parse_listing(&data, page)
    }

    fn dataset(&mut self) -> Result<&[DatasetManga]> {
        if self.dataset.is_none() {
            let mut entries = Vec::new();
            let mut first_page = 1;
            while first_page <= MAX_DATASET_PAGES {
                let requests = (first_page..first_page + DATASET_BATCH_SIZE)
                    .map(|page| {
                        self.client
                            .get(format!("{BASE_URL}/mangas_{page}.json"))
                            .max_body_bytes(2_000_000)
                    })
                    .collect();
                let responses = Client::send_many(requests, DATASET_BATCH_SIZE as u16);
                let mut complete_batch = true;
                for response in responses {
                    let Ok(response) = response.and_then(|response| response.error_for_status())
                    else {
                        complete_batch = false;
                        break;
                    };
                    let page: DatasetPage = response.json()?;
                    if page.list.is_empty() {
                        complete_batch = false;
                        break;
                    }
                    entries.extend(page.list);
                }
                if !complete_batch {
                    break;
                }
                first_page += DATASET_BATCH_SIZE;
            }
            if entries.is_empty() {
                return Err(Error::new("SoraRaw manga dataset is empty"));
            }
            self.dataset = Some(entries);
        }
        Ok(self.dataset.as_deref().unwrap_or_default())
    }

    fn search_dataset(
        &mut self,
        query: &str,
        page: u32,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        let query = query.trim().to_lowercase();
        let genres = selected_values(filters, "genres")
            .into_iter()
            .filter_map(|value| value.parse::<u64>().ok())
            .collect::<BTreeSet<_>>();
        let mode = selected(filters, "mode");
        let content = selected(filters, "content");
        let status = selected(filters, "status");
        let order = selected(filters, "order").unwrap_or("views");

        let mut matches = self
            .dataset()?
            .iter()
            .filter(|entry| entry.dcma.as_deref() != Some("yes"))
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry
                        .alt_names
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                    || entry
                        .author
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            })
            .filter(|entry| {
                genres.is_empty() || entry.genres.iter().any(|genre| genres.contains(genre))
            })
            .filter(|entry| mode.is_none_or(|mode| entry.mode.as_deref() == Some(mode)))
            .filter(|entry| match content {
                Some("adult") => entry.is_adult.as_deref() == Some("yes"),
                Some("general") => entry.is_adult.as_deref() != Some("yes"),
                _ => true,
            })
            .filter(|entry| match status {
                Some("ongoing") => entry.kind.as_deref() == Some("incomplete"),
                Some("complete") => entry.kind.as_deref() == Some("complete"),
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| match order {
            "updated_at" => right.c_published.cmp(&left.c_published),
            "bookmark" => right.number_bookmark.cmp(&left.number_bookmark),
            _ => right.views.cmp(&left.views),
        });

        let page = page.max(1) as usize;
        let start = (page - 1).saturating_mul(SEARCH_PAGE_SIZE);
        let has_next_page = start + SEARCH_PAGE_SIZE < matches.len();
        let entries = matches
            .into_iter()
            .skip(start)
            .take(SEARCH_PAGE_SIZE)
            .map(DatasetManga::into_catalog)
            .collect();
        Ok(Paged::new(entries, has_next_page))
    }
}

impl MangaSource for SoraRawSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page("popular", page)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing_page("latest", page)
    }

    fn listing(
        &mut self,
        listing: &str,
        page: u32,
        _filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        self.listing_page(listing, page)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if query.starts_with("https://") || query.starts_with("http://") {
            if let Some(resolved) = self.handle_url(query)? {
                return Ok(Paged::new(resolved.item.into_iter().collect(), false));
            }
        }
        self.search_dataset(query, page, filters)
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
        Ok(filter_definitions())
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
    dcma: Option<String>,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    number_bookmark: u64,
    #[serde(default)]
    c_published: Option<String>,
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

fn paginate_values(values: &[Value], page: u32) -> Result<Paged<CatalogItem>> {
    let page = page.max(1) as usize;
    let start = (page - 1).saturating_mul(SEARCH_PAGE_SIZE);
    let entries = values
        .iter()
        .skip(start)
        .take(SEARCH_PAGE_SIZE)
        .map(catalog_item)
        .collect::<Result<Vec<_>>>()?;
    Ok(Paged::new(entries, start + SEARCH_PAGE_SIZE < values.len()))
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
            source_order: Some(parsed.len() as i32),
            ..MangaChapter::default()
        });
    }
    if parsed.is_empty() {
        return Err(Error::new("SoraRaw details page has no readable chapters"));
    }
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

fn filter_definitions() -> Vec<FilterDefinition> {
    vec![
        FilterDefinition::MultiSelect {
            id: "genres".to_owned(),
            name: "Genres".to_owned(),
            options: genre_options(),
            default: Vec::new(),
        },
        select_filter(
            "mode",
            "Reading mode",
            &[
                ("All", ""),
                ("Horizontal", "horizontal"),
                ("Vertical", "vertical"),
            ],
        ),
        select_filter(
            "content",
            "Content",
            &[("All", ""), ("General", "general"), ("18+", "adult")],
        ),
        select_filter(
            "status",
            "Status",
            &[
                ("All", ""),
                ("Ongoing", "ongoing"),
                ("Completed", "complete"),
            ],
        ),
        select_filter(
            "order",
            "Sort by",
            &[
                ("Views", "views"),
                ("Recently updated", "updated_at"),
                ("Bookmarks", "bookmark"),
            ],
        ),
    ]
}

fn genre_options() -> Vec<OptionItem> {
    [
        ("Action", "1"),
        ("Fantasy", "4"),
        ("Historical", "5"),
        ("Psychological", "9"),
        ("Sci-Fi", "10"),
        ("Slice of Life", "12"),
        ("Adult", "16"),
        ("Comedy", "17"),
        ("Drama", "18"),
        ("Horror", "20"),
        ("Mystery", "23"),
        ("Romance", "24"),
        ("Seinen", "25"),
        ("Shounen", "26"),
        ("Adventure", "31"),
        ("Ecchi", "33"),
        ("Harem", "34"),
        ("Josei", "35"),
        ("Mature", "37"),
        ("School Life", "39"),
        ("Shoujo", "40"),
        ("Supernatural", "43"),
        ("Isekai", "48"),
        ("BL", "270"),
        ("Original", "271"),
        ("Manga adaptation", "334"),
        ("Romantic comedy", "567"),
        ("Doujinshi", "597"),
        ("Full Color", "511"),
        ("Reincarnation", "527"),
        ("Japanese manga", "1085"),
        ("Overseas manga", "1086"),
    ]
    .into_iter()
    .map(|(label, value)| OptionItem {
        label: label.to_owned(),
        value: value.to_owned(),
    })
    .collect()
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
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        _ => Vec::new(),
    }
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
        assert_eq!(chapters[0].source_order, Some(0));
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
    fn exposes_site_search_filters() {
        let filters = filter_definitions();
        assert_eq!(filters.len(), 5);
        let FilterDefinition::MultiSelect { options, .. } = &filters[0] else {
            panic!("genres must be a multi-select");
        };
        assert!(options.iter().any(|option| option.label == "Action"));
        assert!(options.iter().any(|option| option.label == "BL"));
        assert!(options.iter().any(|option| option.label == "Adult"));
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
