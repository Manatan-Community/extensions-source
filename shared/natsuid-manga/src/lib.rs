use std::{collections::BTreeMap, sync::Mutex};

use chrono::{DateTime, Utc};
use manatan_common::{absolute_url, extract_number, normalize_space, path_key};
use manatan_sdk::{
    client::Client,
    host,
    html::{self, ElementRef, Html, Selector},
    model::{
        CatalogItem, FilterDefinition, MangaChapter, MangaPage, OptionItem, PageContent, Paged,
        SortOption, SortSelection, UrlResolveResult,
    },
    Error, MangaSource, Result,
};
use scraper::Html as ScraperHtml;
use serde::Deserialize;
use serde_json::{json, Value};
use url::{form_urlencoded, Url};

const SORT_VALUES: [(&str, &str); 5] = [
    ("Popular", "popular"),
    ("Rating", "rating"),
    ("Updated", "updated"),
    ("Bookmarked", "bookmarked"),
    ("Title", "title"),
];

const TYPE_VALUES: [(&str, &str); 3] = [
    ("Manga", "manga"),
    ("Manhwa", "manhwa"),
    ("Manhua", "manhua"),
];

const STATUS_VALUES: [(&str, &str); 5] = [
    ("Ongoing", "ongoing"),
    ("Completed", "completed"),
    ("Cancelled", "cancelled"),
    ("On Hiatus", "on-hiatus"),
    ("Unknown", "unknown"),
];

pub trait NatsuIdMangaConfig: Default + 'static {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const LANG: &'static str;
    const CONTENT_RATING: Option<&'static str> = None;

    fn referer(&self) -> String {
        format!("{}/", base_url(Self::BASE_URL))
    }

    fn filter_novels(&self) -> bool {
        true
    }

    fn chapter_anchor_selector(&self) -> &'static str {
        "div a"
    }

    fn chapter_title_selector(&self) -> &'static str {
        "span"
    }

    fn chapter_date_selector(&self) -> &'static str {
        "time"
    }

    fn chapter_date_attribute(&self) -> &'static str {
        "datetime"
    }

    fn page_image_selector(&self) -> &'static str {
        "main .relative section > img"
    }

    fn chapter_list_page_value(&self) -> u32 {
        (host::now_millis().unsigned_abs() % 9_901) as u32 + 99
    }

    fn transform_json_response(&self, source: &str) -> String {
        source.to_owned()
    }
}

pub struct NatsuIdMangaSource<C> {
    client: Client,
    config: C,
    nonce: Mutex<Option<String>>,
}

impl<C: NatsuIdMangaConfig> Default for NatsuIdMangaSource<C> {
    fn default() -> Self {
        Self::new(C::default())
    }
}

impl<C: NatsuIdMangaConfig> NatsuIdMangaSource<C> {
    pub fn new(config: C) -> Self {
        let client = Client::browser().header("Referer", config.referer());
        Self {
            client,
            config,
            nonce: Mutex::new(None),
        }
    }

    fn base_url(&self) -> String {
        base_url(C::BASE_URL)
    }

    fn classify_item(&self, mut item: CatalogItem) -> CatalogItem {
        item.content_rating = C::CONTENT_RATING.map(str::to_owned);
        item
    }

    fn get_html(&self, url: &str) -> Result<(Html, String)> {
        let response = self
            .client
            .get(url)
            .cookies_for(url)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn get_text(&self, url: &str) -> Result<(String, String)> {
        let response = self
            .client
            .get(url)
            .cookies_for(url)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((response.text()?.to_owned(), final_url))
    }

    fn post_form_text(&self, url: &str, form: &[(String, String)]) -> Result<(String, String)> {
        let pairs = form
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let response = self
            .client
            .post(url)
            .cookies_for(url)
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&pairs)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((response.text()?.to_owned(), final_url))
    }

    fn nonce(&self) -> Result<String> {
        if let Some(nonce) = self.nonce.lock().unwrap().clone() {
            return Ok(nonce);
        }

        let url = nonce_url(&self.base_url())?;
        let (body, _) = self.get_text(&url)?;
        let nonce = extract_nonce(&body)
            .ok_or_else(|| Error::new("NatsuId search nonce input was not present"))?;
        *self.nonce.lock().unwrap() = Some(nonce.clone());
        Ok(nonce)
    }

    fn fetch_listing_page(&self, page: u32, sort: SearchSort) -> Result<Paged<CatalogItem>> {
        let form = build_search_form(
            &self.nonce()?,
            "",
            page,
            &SearchFilters {
                sort,
                ..SearchFilters::default()
            },
        );
        let (body, _) = self.post_form_text(&advanced_search_url(&self.base_url())?, &form)?;
        self.parse_listing_payload(&body)
    }

    fn fetch_search_page(
        &self,
        query: &str,
        page: u32,
        filters: &Value,
    ) -> Result<Paged<CatalogItem>> {
        let query = query.trim();
        if query.starts_with("https://") || query.starts_with("http://") {
            if let Some(page) = self.deep_link_page(query)? {
                return Ok(page);
            }
        }

        let parsed_filters = SearchFilters::from_value(filters);
        let form = build_search_form(&self.nonce()?, query, page, &parsed_filters);
        let (body, _) = self.post_form_text(&advanced_search_url(&self.base_url())?, &form)?;
        self.parse_listing_payload(&body)
    }

    fn parse_listing_payload(&self, body: &str) -> Result<Paged<CatalogItem>> {
        let document = html::document(body);
        let slugs = parse_listing_slugs(&document, &self.base_url())?;
        if slugs.is_empty() {
            return Ok(Paged::new(Vec::new(), false));
        }
        let details_url = rest_manga_list_url(&self.base_url(), &slugs)?;
        let (json_source, _) = self.get_text(&details_url)?;
        let mapped = parse_rest_manga_list(
            &self.config.transform_json_response(&json_source),
            &self.base_url(),
            C::LANG,
            self.config.filter_novels(),
        )?;
        let by_slug = mapped
            .into_iter()
            .filter_map(|item| {
                let slug = item
                    .extra
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(str::to_owned)?;
                Some((slug, item))
            })
            .collect::<BTreeMap<_, _>>();
        let entries = slugs
            .into_iter()
            .filter_map(|slug| by_slug.get(&slug).cloned())
            .map(|item| self.classify_item(item))
            .collect::<Vec<_>>();
        let has_next_page = listing_has_next_page(&document)?;
        Ok(Paged::new(entries, has_next_page))
    }

    fn deep_link_page(&self, query: &str) -> Result<Option<Paged<CatalogItem>>> {
        let Some(slug) = slug_from_candidate(C::BASE_URL, query)? else {
            return Ok(None);
        };
        let details_url = rest_manga_list_url(&self.base_url(), std::slice::from_ref(&slug))?;
        let (json_source, _) = self.get_text(&details_url)?;
        let mut items = parse_rest_manga_list(
            &self.config.transform_json_response(&json_source),
            &self.base_url(),
            C::LANG,
            self.config.filter_novels(),
        )?;
        let item = items
            .drain(..)
            .next()
            .ok_or_else(|| Error::new("NatsuId deep link did not resolve to a manga"))?;
        Ok(Some(Paged::new(vec![self.classify_item(item)], false)))
    }

    fn detail_item(&self, item: &CatalogItem) -> Result<CatalogItem> {
        if let Some(id) = manga_id(item) {
            let url = rest_manga_item_url(&self.base_url(), id)?;
            let (json_source, _) = self.get_text(&url)?;
            return parse_rest_manga_details(
                &self.config.transform_json_response(&json_source),
                &self.base_url(),
                C::LANG,
            )
            .map(|item| self.classify_item(item));
        }

        if let Some(slug) = item
            .extra
            .get("slug")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| slug_from_item(C::BASE_URL, item).ok().flatten())
        {
            let url = rest_manga_list_url(&self.base_url(), &[slug])?;
            let (json_source, _) = self.get_text(&url)?;
            let items = parse_rest_manga_list(
                &self.config.transform_json_response(&json_source),
                &self.base_url(),
                C::LANG,
                self.config.filter_novels(),
            )?;
            return items
                .into_iter()
                .next()
                .map(|item| self.classify_item(item))
                .ok_or_else(|| Error::new("NatsuId details payload did not contain a manga"));
        }

        Err(Error::new(
            "NatsuId item had neither an id nor a manga slug",
        ))
    }

    fn manga_id_and_url(&self, item: &CatalogItem) -> Result<(String, String)> {
        let item_url = item_url(C::BASE_URL, item)?;
        if let Some(id) = manga_id(item) {
            return Ok((id.to_string(), item_url));
        }

        if let Some(slug) = slug_from_item(C::BASE_URL, item)? {
            let url = rest_manga_list_url(&self.base_url(), &[slug])?;
            let (json_source, _) = self.get_text(&url)?;
            let items = parse_rest_manga_list(
                &self.config.transform_json_response(&json_source),
                &self.base_url(),
                C::LANG,
                false,
            )?;
            if let Some(found) = items.into_iter().next() {
                if let Some(id) = manga_id(&found) {
                    return Ok((id.to_string(), item_url));
                }
            }
        }

        let (document, _) = self.get_html(&item_url)?;
        let id = extract_manga_id_from_detail_page(&document)?
            .ok_or_else(|| Error::new("NatsuId detail page did not expose manga_id"))?;
        Ok((id, item_url))
    }
}

impl<C: NatsuIdMangaConfig> MangaSource for NatsuIdMangaSource<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.fetch_listing_page(page, SearchSort::popular())
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.fetch_listing_page(page, SearchSort::latest())
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        self.fetch_search_page(query, page, filters)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        self.detail_item(&item)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let (manga_id, item_url) = self.manga_id_and_url(&item)?;
        let chapter_page = self.config.chapter_list_page_value();
        let url = chapter_list_url(&self.base_url(), &manga_id, chapter_page)?;
        let (body, _) = self.get_text(&url)?;
        parse_chapters_html(
            &html::document(&body),
            &item_url,
            self.config.chapter_anchor_selector(),
            self.config.chapter_title_selector(),
            self.config.chapter_date_selector(),
            self.config.chapter_date_attribute(),
            C::LANG,
        )
    }

    fn pages(&mut self, _item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let chapter_url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (document, final_url) = self.get_html(chapter_url)?;
        parse_pages_html(&document, &final_url, self.config.page_image_selector())
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        let genres = match genre_filter_url(&self.base_url()) {
            Ok(url) => self
                .get_text(&url)
                .ok()
                .and_then(|(body, _)| {
                    parse_genres_json(&self.config.transform_json_response(&body)).ok()
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Ok(build_filter_definitions(&genres))
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(item_url(C::BASE_URL, item)?))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let candidate = chapter.url.as_deref().unwrap_or(&chapter.key);
        Ok(Some(absolute_url(C::BASE_URL, candidate)?))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let Some(slug) = slug_from_candidate(C::BASE_URL, candidate)? else {
            return Ok(None);
        };
        let item_url = format!("{}/manga/{slug}/", self.base_url());
        let mut item = CatalogItem {
            key: path_key(C::BASE_URL, &item_url)?,
            url: Some(item_url.clone()),
            language: Some(C::LANG.to_owned()),
            ..CatalogItem::default()
        };
        item.content_rating = C::CONTENT_RATING.map(str::to_owned);
        item.extra.insert("slug".to_owned(), json!(slug));
        let mut result = UrlResolveResult {
            item: Some(item),
            ..UrlResolveResult::default()
        };
        let parsed = Url::parse(candidate).map_err(url_error)?;
        if parsed
            .path_segments()
            .map(|segments| segments.count() > 2)
            .unwrap_or(false)
        {
            let chapter_url = parsed.to_string();
            result.chapter_key = Some(chapter_url.clone());
            result.manga_chapter = Some(MangaChapter {
                key: chapter_url.clone(),
                url: Some(chapter_url),
                language: Some(C::LANG.to_owned()),
                ..MangaChapter::default()
            });
        }
        Ok(Some(result))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchSort {
    pub value: String,
    pub ascending: bool,
}

impl SearchSort {
    pub fn popular() -> Self {
        Self {
            value: "popular".to_owned(),
            ascending: false,
        }
    }

    pub fn latest() -> Self {
        Self {
            value: "updated".to_owned(),
            ascending: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchFilters {
    pub genre_inclusion_mode: String,
    pub genre_exclusion_mode: String,
    pub included_genres: Vec<String>,
    pub excluded_genres: Vec<String>,
    pub project_only: bool,
    pub types: Vec<String>,
    pub statuses: Vec<String>,
    pub sort: SearchSort,
}

impl SearchFilters {
    pub fn from_value(filters: &Value) -> Self {
        Self {
            genre_inclusion_mode: selected_mode(filters, "genre_inclusion_mode"),
            genre_exclusion_mode: selected_mode(filters, "genre_exclusion_mode"),
            included_genres: selected_values(filters, "genre_include"),
            excluded_genres: selected_values(filters, "genre_exclude"),
            project_only: selected_bool(filters, "project"),
            types: selected_values(filters, "type"),
            statuses: selected_values(filters, "status"),
            sort: selected_sort(filters),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct GenreOption {
    pub name: String,
    pub slug: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RestTerm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    taxonomy: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RenderedField {
    #[serde(default)]
    rendered: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FeaturedMedia {
    #[serde(default, rename = "source_url")]
    source_url: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct EmbeddedFields {
    #[serde(default, rename = "wp:featuredmedia")]
    featured_media: Vec<FeaturedMedia>,
    #[serde(default, rename = "wp:term")]
    terms: Vec<Vec<RestTerm>>,
}

impl EmbeddedFields {
    fn terms(&self, taxonomy: &str) -> Vec<String> {
        self.terms
            .iter()
            .find(|group| group.first().map(|term| term.taxonomy.as_str()) == Some(taxonomy))
            .map(|group| {
                group
                    .iter()
                    .map(|term| term.name.trim())
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize)]
struct RestManga {
    id: u64,
    slug: String,
    title: RenderedField,
    content: RenderedField,
    #[serde(default, rename = "_embedded")]
    embedded: EmbeddedFields,
}

impl RestManga {
    fn is_novel(&self) -> bool {
        self.embedded
            .terms("type")
            .iter()
            .any(|term| term.eq_ignore_ascii_case("Novel"))
    }

    fn to_catalog_item(&self, base: &str, lang: &str) -> Result<CatalogItem> {
        let url = format!("{}/manga/{}/", base.trim_end_matches('/'), self.slug);
        let mut item = CatalogItem::new(path_key(base, &url)?, html_text(&self.title.rendered));
        item.url = Some(url);
        item.cover = self
            .embedded
            .featured_media
            .first()
            .map(|media| media.source_url.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .map(Into::into);
        let description = html_text(&self.content.rendered);
        if !description.is_empty() {
            item.description = Some(description);
        }
        item.authors = self.embedded.terms("series-author");
        item.artists = self.embedded.terms("artist");
        item.tags = merged_terms(&self.embedded.terms("genre"), &self.embedded.terms("type"));
        item.status = Some(json!(status_from_terms(&self.embedded.terms("status"))));
        item.initialized = true;
        item.language = Some(lang.to_owned());
        item.extra.insert("mangaId".to_owned(), json!(self.id));
        item.extra.insert("slug".to_owned(), json!(self.slug));
        Ok(item)
    }
}

pub fn build_search_form(
    nonce: &str,
    query: &str,
    page: u32,
    filters: &SearchFilters,
) -> Vec<(String, String)> {
    vec![
        ("nonce".to_owned(), nonce.to_owned()),
        (
            "inclusion".to_owned(),
            non_empty_or(&filters.genre_inclusion_mode, "OR"),
        ),
        (
            "exclusion".to_owned(),
            non_empty_or(&filters.genre_exclusion_mode, "OR"),
        ),
        ("page".to_owned(), page.max(1).to_string()),
        (
            "genre".to_owned(),
            serde_json::to_string(&filters.included_genres).unwrap_or_else(|_| "[]".to_owned()),
        ),
        (
            "genre_exclude".to_owned(),
            serde_json::to_string(&filters.excluded_genres).unwrap_or_else(|_| "[]".to_owned()),
        ),
        ("author".to_owned(), "[]".to_owned()),
        ("artist".to_owned(), "[]".to_owned()),
        (
            "project".to_owned(),
            if filters.project_only { "1" } else { "0" }.to_owned(),
        ),
        (
            "type".to_owned(),
            serde_json::to_string(&filters.types).unwrap_or_else(|_| "[]".to_owned()),
        ),
        (
            "status".to_owned(),
            serde_json::to_string(&filters.statuses).unwrap_or_else(|_| "[]".to_owned()),
        ),
        (
            "order".to_owned(),
            if filters.sort.ascending {
                "asc"
            } else {
                "desc"
            }
            .to_owned(),
        ),
        (
            "orderby".to_owned(),
            non_empty_or(&filters.sort.value, "popular"),
        ),
        ("query".to_owned(), query.trim().to_owned()),
    ]
}

pub fn build_filter_definitions(genres: &[GenreOption]) -> Vec<FilterDefinition> {
    let mut filters = vec![
        FilterDefinition::Sort {
            id: "sort".to_owned(),
            name: "Sort".to_owned(),
            options: SORT_VALUES
                .iter()
                .map(|(label, value)| SortOption {
                    label: (*label).to_owned(),
                    value: (*value).to_owned(),
                })
                .collect(),
            default: Some(SortSelection {
                index: 0,
                ascending: false,
            }),
        },
        check_group("type", "Type", &TYPE_VALUES),
        check_group("status", "Status", &STATUS_VALUES),
        FilterDefinition::CheckBox {
            id: "project".to_owned(),
            name: "Project Only".to_owned(),
            default: false,
        },
        FilterDefinition::Select {
            id: "genre_inclusion_mode".to_owned(),
            name: "Genre Inclusion Mode".to_owned(),
            options: option_items(&[("OR", "OR"), ("AND", "AND")]),
            default_index: 0,
        },
        FilterDefinition::Select {
            id: "genre_exclusion_mode".to_owned(),
            name: "Genre Exclusion Mode".to_owned(),
            options: option_items(&[("OR", "OR"), ("AND", "AND")]),
            default_index: 0,
        },
    ];

    if !genres.is_empty() {
        let options = genres
            .iter()
            .map(|genre| (genre.name.as_str(), genre.slug.as_str()))
            .collect::<Vec<_>>();
        filters.push(check_group("genre_include", "Included Genres", &options));
        filters.push(check_group("genre_exclude", "Excluded Genres", &options));
    }

    filters
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

pub fn parse_genres_json(source: &str) -> Result<Vec<GenreOption>> {
    let terms = serde_json::from_str::<Vec<RestTerm>>(source).map_err(json_error)?;
    Ok(terms
        .into_iter()
        .filter(|term| !term.slug.trim().is_empty() && !term.name.trim().is_empty())
        .map(|term| GenreOption {
            name: term.name.trim().to_owned(),
            slug: term.slug.trim().to_owned(),
        })
        .collect())
}

pub fn parse_rest_manga_list(
    source: &str,
    base: &str,
    lang: &str,
    filter_novels: bool,
) -> Result<Vec<CatalogItem>> {
    let mangas = serde_json::from_str::<Vec<RestManga>>(source).map_err(json_error)?;
    mangas
        .into_iter()
        .filter(|manga| !filter_novels || !manga.is_novel())
        .map(|manga| manga.to_catalog_item(base, lang))
        .collect()
}

pub fn parse_rest_manga_details(source: &str, base: &str, lang: &str) -> Result<CatalogItem> {
    let manga = serde_json::from_str::<RestManga>(source).map_err(json_error)?;
    if manga.is_novel() {
        return Err(Error::new("NatsuId details payload resolved to a novel"));
    }
    manga.to_catalog_item(base, lang)
}

pub fn parse_listing_slugs(document: &Html, base: &str) -> Result<Vec<String>> {
    let selector = selector("a[href*='/manga/']")?;
    let mut slugs = Vec::new();
    for anchor in document.select(&selector) {
        if !has_direct_image(anchor)? {
            continue;
        }
        let Some(href) = attribute(anchor, "href") else {
            continue;
        };
        let Some(slug) = slug_from_url(&absolute_url(base, &href)?)? else {
            continue;
        };
        if !slugs.contains(&slug) {
            slugs.push(slug);
        }
    }
    Ok(slugs)
}

pub fn listing_has_next_page(document: &Html) -> Result<bool> {
    let button_selector = selector("button")?;
    let svg_selector = selector("svg")?;
    Ok(document
        .select(&button_selector)
        .any(|button| button.select(&svg_selector).next().is_some()))
}

pub fn parse_chapters_html(
    document: &Html,
    base: &str,
    anchor_selector: &str,
    title_selector: &str,
    date_selector: &str,
    date_attribute: &str,
    lang: &str,
) -> Result<Vec<MangaChapter>> {
    let anchor_selector = selector(anchor_selector)?;
    let time_selector = selector("time")?;
    let title_selector = selector(title_selector)?;
    let date_selector = selector(date_selector)?;

    document
        .select(&anchor_selector)
        .filter(|anchor| anchor.select(&time_selector).next().is_some())
        .map(|anchor| {
            let href = attribute(anchor, "href")
                .ok_or_else(|| Error::new("NatsuId chapter entry had no href"))?;
            let title = anchor
                .select(&title_selector)
                .next()
                .map(text)
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::new("NatsuId chapter entry had no title"))?;
            let mut url = Url::parse(&absolute_url(base, &href)?).map_err(url_error)?;
            url.set_query(Some("style=list"));
            let date_uploaded = anchor
                .select(&date_selector)
                .next()
                .and_then(|element| attribute(element, date_attribute))
                .and_then(|value| parse_rfc3339(&value));
            Ok(MangaChapter {
                key: path_key(base, url.as_str())?,
                title: Some(title.clone()),
                chapter_number: extract_number(&title),
                date_uploaded,
                language: Some(lang.to_owned()),
                url: Some(url.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

pub fn parse_pages_html(
    document: &Html,
    chapter_url: &str,
    image_selector: &str,
) -> Result<Vec<MangaPage>> {
    let selector = selector(image_selector)?;
    document
        .select(&selector)
        .filter_map(|image| image_source(image).transpose())
        .map(|url| {
            url.and_then(|url| {
                absolute_url(chapter_url, &url).map(|url| MangaPage {
                    content: PageContent::Url {
                        url,
                        context: Some(page_headers(chapter_url)),
                    },
                    headers: page_headers(chapter_url),
                    ..MangaPage::default()
                })
            })
        })
        .collect()
}

pub fn extract_nonce(source: &str) -> Option<String> {
    let document = html::document(source);
    let selector = html::selector("input[name='search_nonce']").ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|element| attribute(element, "value"))
}

pub fn extract_manga_id_from_detail_page(document: &Html) -> Result<Option<String>> {
    let selector = selector("#gallery-list, [hx-get*='manga_id=']")?;
    Ok(document.select(&selector).find_map(|element| {
        attribute(element, "hx-get").and_then(|value| {
            value
                .split("manga_id=")
                .nth(1)
                .and_then(|part| part.split('&').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    }))
}

pub fn advanced_search_url(base: &str) -> Result<String> {
    joined_url(base, "/wp-admin/admin-ajax.php?action=advanced_search")
}

pub fn nonce_url(base: &str) -> Result<String> {
    joined_url(
        base,
        "/wp-admin/admin-ajax.php?type=search_form&action=get_nonce",
    )
}

pub fn genre_filter_url(base: &str) -> Result<String> {
    joined_url(
        base,
        "/wp-json/wp/v2/genre?per_page=100&page=1&orderby=count&order=desc",
    )
}

pub fn rest_manga_list_url(base: &str, slugs: &[String]) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    url.set_path("/wp-json/wp/v2/manga");
    let mut query = form_urlencoded::Serializer::new(String::new());
    for slug in slugs {
        query.append_pair("slug[]", slug);
    }
    query.append_pair("per_page", &(slugs.len() + 1).to_string());
    let mut query = query.finish();
    if !query.is_empty() {
        query.push('&');
    }
    query.push_str("_embed");
    url.set_query(Some(&query));
    Ok(url.to_string())
}

pub fn rest_manga_item_url(base: &str, manga_id: u64) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    url.set_path(&format!("/wp-json/wp/v2/manga/{manga_id}"));
    url.set_query(Some("_embed"));
    Ok(url.to_string())
}

pub fn chapter_list_url(base: &str, manga_id: &str, page_value: u32) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    url.set_path("/wp-admin/admin-ajax.php");
    let mut query = url.query_pairs_mut();
    query
        .append_pair("manga_id", manga_id)
        .append_pair("page", &page_value.max(4).to_string())
        .append_pair("action", "chapter_list");
    drop(query);
    Ok(url.to_string())
}

fn base_url(candidate: &str) -> String {
    candidate.trim_end_matches('/').to_owned()
}

fn slug_from_candidate(base: &str, candidate: &str) -> Result<Option<String>> {
    let parsed = Url::parse(candidate).map_err(url_error)?;
    let base = Url::parse(base).map_err(url_error)?;
    if parsed.scheme() != base.scheme()
        || parsed.host_str() != base.host_str()
        || parsed.port_or_known_default() != base.port_or_known_default()
    {
        return Ok(None);
    }
    slug_from_url(parsed.as_str())
}

fn slug_from_item(base: &str, item: &CatalogItem) -> Result<Option<String>> {
    if let Some(slug) = item.extra.get("slug").and_then(Value::as_str) {
        return Ok(Some(slug.to_owned()));
    }
    let candidate = item.url.as_deref().unwrap_or(&item.key);
    let absolute = absolute_url(base, candidate)?;
    slug_from_url(&absolute)
}

fn slug_from_url(candidate: &str) -> Result<Option<String>> {
    let parsed = Url::parse(candidate).map_err(url_error)?;
    let segments = parsed
        .path_segments()
        .map(|parts| parts.collect::<Vec<_>>());
    let Some(segments) = segments else {
        return Ok(None);
    };
    if segments.len() >= 2 && segments[0] == "manga" && !segments[1].is_empty() {
        return Ok(Some(segments[1].to_owned()));
    }
    Ok(None)
}

fn item_url(base: &str, item: &CatalogItem) -> Result<String> {
    let candidate = item.url.as_deref().unwrap_or(&item.key);
    absolute_url(base, candidate)
}

fn manga_id(item: &CatalogItem) -> Option<u64> {
    item.extra
        .get("mangaId")
        .or_else(|| item.extra.get("id"))
        .and_then(Value::as_u64)
}

fn has_direct_image(element: ElementRef<'_>) -> Result<bool> {
    let image_selector = selector("img")?;
    Ok(element
        .children()
        .filter_map(ElementRef::wrap)
        .any(|child| {
            child.select(&image_selector).next().is_some() || child.value().name() == "img"
        }))
}

fn image_source(element: ElementRef<'_>) -> Result<Option<String>> {
    for name in ["src", "data-src", "data-lazy-src"] {
        if let Some(value) = attribute(element, name).filter(|value| !value.is_empty()) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}

fn text(element: ElementRef<'_>) -> String {
    html::text(element)
}

fn attribute(element: ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn merged_terms(primary: &[String], secondary: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    for value in primary.iter().chain(secondary.iter()) {
        if !merged
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(value))
        {
            merged.push(value.clone());
        }
    }
    merged
}

fn status_from_terms(terms: &[String]) -> &'static str {
    let normalized = terms
        .iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if normalized.iter().any(|term| term.contains("ongoing")) {
        "ongoing"
    } else if normalized.iter().any(|term| term.contains("completed")) {
        "completed"
    } else if normalized.iter().any(|term| term.contains("cancelled")) {
        "cancelled"
    } else if normalized.iter().any(|term| term.contains("hiatus")) {
        "on_hiatus"
    } else {
        "unknown"
    }
}

fn html_text(source: &str) -> String {
    let document = ScraperHtml::parse_fragment(source);
    let text = document.root_element().text().collect::<Vec<_>>().join(" ");
    normalize_space(&text)
}

fn page_headers(referer: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(String::from("Referer"), referer.to_owned())])
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc).timestamp_millis())
}

fn option_items(values: &[(&str, &str)]) -> Vec<OptionItem> {
    values
        .iter()
        .map(|(label, value)| OptionItem {
            label: (*label).to_owned(),
            value: (*value).to_owned(),
        })
        .collect()
}

fn selected_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::Object(values)) => values
            .iter()
            .filter(|(_, selected)| selected.as_bool().unwrap_or(false))
            .map(|(value, _)| value.clone())
            .collect(),
        Some(Value::String(value)) if !value.trim().is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn selected_bool(filters: &Value, key: &str) -> bool {
    match filters.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_u64() == Some(1),
        Some(Value::String(value)) => matches!(value.trim(), "1" | "true" | "on"),
        _ => false,
    }
}

fn selected_mode(filters: &Value, key: &str) -> String {
    filters
        .get(key)
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(value) => value
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
        .filter(|value| matches!(value.as_str(), "OR" | "AND"))
        .unwrap_or_else(|| "OR".to_owned())
}

fn selected_sort(filters: &Value) -> SearchSort {
    let Some(value) = filters.get("sort") else {
        return SearchSort::popular();
    };

    if let Some(raw) = value.as_str() {
        return SearchSort {
            value: raw.to_owned(),
            ascending: false,
        };
    }

    if let Some(object) = value.as_object() {
        if let Some(raw) = object.get("value").and_then(Value::as_str) {
            return SearchSort {
                value: raw.to_owned(),
                ascending: object
                    .get("ascending")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
        }
        if let Some(index) = object.get("index").and_then(Value::as_u64) {
            let value = SORT_VALUES
                .get(index as usize)
                .map(|(_, value)| (*value).to_owned())
                .unwrap_or_else(|| "popular".to_owned());
            return SearchSort {
                value,
                ascending: object
                    .get("ascending")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
        }
    }

    SearchSort::popular()
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn joined_url(base: &str, path_and_query: &str) -> Result<String> {
    let mut url = Url::parse(base).map_err(url_error)?;
    let path_and_query = path_and_query.trim_start_matches('/');
    let joined = format!("/{}", path_and_query);
    let (path, query) = joined.split_once('?').unwrap_or((&joined, ""));
    url.set_path(path);
    if query.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(query));
    }
    Ok(url.to_string())
}

fn url_error(error: impl ToString) -> Error {
    Error::new(error.to_string())
}

fn json_error(error: impl ToString) -> Error {
    Error::new(format!("NatsuId JSON decode error: {}", error.to_string()))
}
