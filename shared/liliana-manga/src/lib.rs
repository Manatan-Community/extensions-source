use std::{collections::BTreeMap, marker::PhantomData};

use manatan_common::{absolute_url, attr, canonical_url, first_attr, path_key, text};
use manatan_sdk::{
    client::Client,
    html::{self, ElementRef, Html, Selector},
    CatalogItem, Error, FilterDefinition, MangaChapter, MangaPage, MangaSource, OptionItem,
    PageContent, Paged, Result, UrlResolveResult,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

pub trait LilianaConfig: 'static {
    const BASE_URL: &'static str;
    const LANG: &'static str = "ja";
    const USES_POST_SEARCH: bool = false;
    const CONTENT_RATING: Option<&'static str> = None;

    fn popular_path(page: u32) -> String {
        format!("/ranking/week/{}", page.max(1))
    }

    fn latest_path(page: u32) -> String {
        format!("/all-manga/{}/", page.max(1))
    }

    fn latest_query() -> &'static [(&'static str, &'static str)] {
        &[("sort", "last_update"), ("status", "0")]
    }

    fn search_path(page: u32) -> String {
        format!("/search/{}/", page.max(1))
    }

    fn filter_path(page: u32) -> String {
        format!("/filter/{}/", page.max(1))
    }

    fn filters_path() -> &'static str {
        "/filter"
    }

    fn post_search_path() -> &'static str {
        "/ajax/search"
    }

    fn filter_note() -> &'static str {
        "Ignored if using text search."
    }
}

pub struct LilianaMangaSource<C: LilianaConfig> {
    client: Client,
    _config: PhantomData<C>,
}

impl<C: LilianaConfig> Default for LilianaMangaSource<C> {
    fn default() -> Self {
        Self {
            client: Client::browser().header("Referer", format!("{}/", base_url::<C>())),
            _config: PhantomData,
        }
    }
}

impl<C: LilianaConfig> LilianaMangaSource<C> {
    fn get_document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn post_form_text(
        &self,
        url: &str,
        headers: &[(&str, String)],
        form: &[(&str, &str)],
    ) -> Result<(String, String)> {
        let mut request = self.client.post(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let response = request.form(form).send()?.error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((response.text()?.to_owned(), final_url))
    }

    fn get_text_with_headers(&self, url: &str, headers: &[(&str, String)]) -> Result<String> {
        let mut request = self.client.get(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        Ok(request.send()?.error_for_status()?.text()?.to_owned())
    }

    fn item_url_for(&self, item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        canonical_url(C::BASE_URL, candidate)
    }

    fn classify_item(&self, mut item: CatalogItem) -> CatalogItem {
        item.content_rating = C::CONTENT_RATING.map(str::to_owned);
        item
    }

    fn classify_page(&self, mut page: Paged<CatalogItem>) -> Paged<CatalogItem> {
        for item in &mut page.entries {
            item.content_rating = C::CONTENT_RATING.map(str::to_owned);
        }
        page
    }
}

impl<C: LilianaConfig> MangaSource for LilianaMangaSource<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = popular_url::<C>(page)?;
        let (document, final_url) = self.get_document(&url)?;
        parse_catalog_html(&document, &final_url).map(|page| self.classify_page(page))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = latest_url::<C>(page)?;
        let (document, final_url) = self.get_document(&url)?;
        parse_catalog_html(&document, &final_url).map(|page| self.classify_page(page))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if !query.trim().is_empty() && C::USES_POST_SEARCH {
            let url = absolute_url(C::BASE_URL, C::post_search_path())?;
            let host = url_host(&base_url::<C>())?;
            let (payload, final_url) = self.post_form_text(
                &url,
                &[
                    (
                        "Accept",
                        "application/json, text/javascript, */*; q=0.01".to_owned(),
                    ),
                    ("Host", host),
                    ("Origin", base_url::<C>()),
                    ("X-Requested-With", "XMLHttpRequest".to_owned()),
                ],
                &[("search", query)],
            )?;
            let payload: SearchResponseDto = serde_json::from_str(&payload).map_err(json_error)?;
            return parse_post_search_json(&payload, &final_url)
                .map(|page| self.classify_page(page));
        }

        let url = if query.trim().is_empty() {
            filter_url::<C>(page, filters)?
        } else {
            search_url::<C>(query, page)?
        };
        let (document, final_url) = self.get_document(&url)?;
        parse_catalog_html(&document, &final_url).map(|page| self.classify_page(page))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = self.item_url_for(&item)?;
        let (document, final_url) = self.get_document(&url)?;
        let mut parsed = parse_details_html(&document, &final_url)?;
        parsed.key = item.key;
        parsed.url = Some(final_url);
        Ok(self.classify_item(parsed))
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let url = self.item_url_for(&item)?;
        let (document, final_url) = self.get_document(&url)?;
        parse_chapters_html(&document, &final_url)
    }

    fn pages(&mut self, _item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let chapter_url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (document, final_url) = self.get_document(chapter_url)?;
        let inline_pages = parse_page_list_html(&document, &final_url)?;
        if !inline_pages.is_empty() {
            return Ok(inline_pages);
        }

        let chapter_id = extract_chapter_id(document.html().as_str())?;
        let url = absolute_url(C::BASE_URL, &format!("/ajax/image/list/chap/{chapter_id}"))?;
        let host = url_host(&base_url::<C>())?;
        let payload = self.get_text_with_headers(
            &url,
            &[
                (
                    "Accept",
                    "application/json, text/javascript, */*; q=0.01".to_owned(),
                ),
                ("Host", host),
                ("Referer", final_url.clone()),
                ("X-Requested-With", "XMLHttpRequest".to_owned()),
            ],
        )?;
        let response: PageListResponseDto = serde_json::from_str(&payload).map_err(json_error)?;
        if !response.status {
            return Err(Error::new(
                response
                    .msg
                    .unwrap_or_else(|| "Liliana page list request failed".to_owned()),
            ));
        }
        let ajax_document = html::document(&response.html);
        parse_page_list_html(&ajax_document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        let url = absolute_url(C::BASE_URL, C::filters_path())?;
        let Ok((document, _)) = self.get_document(&url) else {
            return Ok(vec![FilterDefinition::Header {
                name: "Reset filters to retry loading live metadata.".into(),
            }]);
        };
        let metadata = parse_filter_metadata(&document)?;
        Ok(build_filter_definitions(&metadata, C::filter_note()))
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(self.item_url_for(item)?))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        let candidate = chapter.url.as_deref().unwrap_or(&chapter.key);
        Ok(Some(canonical_url(C::BASE_URL, candidate)?))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        resolve_url(C::BASE_URL, C::LANG, candidate).map(|result| {
            result.map(|mut result| {
                result.item = result.item.map(|item| self.classify_item(item));
                result
            })
        })
    }
}

pub fn popular_url<C: LilianaConfig>(page: u32) -> Result<String> {
    build_url::<C>(&C::popular_path(page.max(1)), &[])
}

pub fn latest_url<C: LilianaConfig>(page: u32) -> Result<String> {
    build_url::<C>(&C::latest_path(page.max(1)), C::latest_query())
}

pub fn search_url<C: LilianaConfig>(query: &str, page: u32) -> Result<String> {
    let mut url = Url::parse(&base_url::<C>()).map_err(url_error)?;
    url.set_path(&C::search_path(page.max(1)));
    url.query_pairs_mut().append_pair("keyword", query);
    Ok(url.to_string())
}

pub fn filter_url<C: LilianaConfig>(page: u32, filters: &Value) -> Result<String> {
    let mut url = Url::parse(&base_url::<C>()).map_err(url_error)?;
    url.set_path(&C::filter_path(page.max(1)));
    let mut pairs = Vec::new();
    append_csv_filter(&mut pairs, filters, "genres");
    append_csv_filter(&mut pairs, filters, "notGenres");
    append_scalar_filter(&mut pairs, filters, "type");
    append_scalar_filter(&mut pairs, filters, "status");
    append_scalar_filter(&mut pairs, filters, "sort");
    append_scalar_filter(&mut pairs, filters, "chapter_count");
    append_scalar_filter(&mut pairs, filters, "sex");
    if !pairs.is_empty() {
        let mut serializer = url.query_pairs_mut();
        for (key, value) in pairs {
            serializer.append_pair(&key, &value);
        }
    }
    Ok(url.to_string())
}

pub fn parse_catalog_html(document: &Html, base: &str) -> Result<Paged<CatalogItem>> {
    let entries = select_all(
        document,
        "div#main div.grid > div, div.grid.gtc-f141a > div",
    )?
    .into_iter()
    .filter_map(|element| parse_card(element, base).ok())
    .collect();
    Ok(Paged::new(entries, has_next_page(document)?))
}

pub fn parse_details_html(document: &Html, base: &str) -> Result<CatalogItem> {
    let title = first_text(
        document.root_element(),
        &selector("article.a2 header h1, .a2 header h1")?,
    )
    .ok_or_else(|| Error::new("Liliana details page has no title"))?;
    let mut item = CatalogItem::new(path_key(base, base)?, title);
    item.url = Some(base.to_owned());
    item.cover = first_element(document, ".a1 figure img")
        .map(|image| image_url(image, base))
        .transpose()?
        .flatten()
        .map(Into::into);
    item.description = first_text(
        document.root_element(),
        &selector("#syn-target, div#syn-target")?,
    );
    item.tags = text_list(document, ".a2 a[rel='tag'].label")?;
    if let Some(author) = first_text(
        document.root_element(),
        &selector("div.y6x11p i.fas.fa-user + span.dt")?,
    )
    .filter(|value| !value.eq_ignore_ascii_case("updating"))
    {
        item.authors.push(author);
    }
    item.status = first_text(
        document.root_element(),
        &selector("div.y6x11p i.fas.fa-rss + span.dt")?,
    )
    .map(|value| json!(parse_status(&value)));
    item.initialized = true;
    Ok(item)
}

pub fn parse_chapters_html(document: &Html, base: &str) -> Result<Vec<MangaChapter>> {
    select_all(document, "ul > li.chapter")?
        .into_iter()
        .map(|element| parse_chapter(element, base))
        .collect()
}

pub fn parse_page_list_html(document: &Html, chapter_url: &str) -> Result<Vec<MangaPage>> {
    let indexed = select_all(document, "div.separator[data-index]")?;
    if indexed.is_empty() {
        return select_all(document, "div.separator")?
            .into_iter()
            .filter_map(|element| parse_page_image_url(element, chapter_url).transpose())
            .enumerate()
            .map(|(index, url)| page_from_url(index, &url?))
            .collect();
    }

    let mut pages: Vec<(usize, MangaPage)> = indexed
        .into_iter()
        .filter_map(|element| {
            let index = attr(element, "data-index")?.parse::<usize>().ok()?;
            let url = parse_page_image_url(element, chapter_url).ok()??;
            Some(page_from_url(index, &url).map(|page| (index, page)))
        })
        .collect::<Result<Vec<_>>>()?;
    pages.sort_by_key(|(index, _)| *index);
    Ok(pages.into_iter().map(|(_, page)| page).collect())
}

pub fn extract_chapter_id(source: &str) -> Result<String> {
    let regex = Regex::new(r"const\s+CHAPTER_ID\s*=\s*(\d+)")
        .expect("chapter id regex should always compile");
    regex
        .captures(source)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| Error::new("Liliana chapter page has no CHAPTER_ID"))
}

pub fn parse_filter_metadata(document: &Html) -> Result<FilterMetadata> {
    let root = document.root_element();
    let genres = select_all(document, ".advanced-genres .advance-item")?
        .into_iter()
        .filter_map(|element| {
            let label = first_text(element, &selector("label").ok()?)?;
            let value = first_attr(element, &selector("span").ok()?, "data-genre")?;
            Some(option(value, label))
        })
        .collect();

    Ok(FilterMetadata {
        genre_title: first_text(
            root,
            &selector(".advanced-genres > h3, .advanced-genres > h3.box-title")?,
        )
        .unwrap_or_else(|| "Genres".to_owned()),
        genres,
        type_filter: parse_select_filter(document, "select-type", ".select-type")?,
        chapter_count: parse_select_filter(document, "select-count", ".select-count")?,
        status: parse_select_filter(document, "select-status", ".select-status")?,
        gender: parse_select_filter(document, "select-gender", ".select-gender")?,
        sort: parse_select_filter(document, "select-sort", ".select-sort")?,
    })
}

pub fn build_filter_definitions(metadata: &FilterMetadata, note: &str) -> Vec<FilterDefinition> {
    let mut filters = Vec::new();
    if !note.is_empty() {
        filters.push(FilterDefinition::Header {
            name: note.to_owned(),
        });
        filters.push(FilterDefinition::Separator);
    }
    if !metadata.genres.is_empty() {
        let values = metadata
            .genres
            .iter()
            .map(|option| (option.label.as_str(), option.value.as_str()))
            .collect::<Vec<_>>();
        filters.push(FilterDefinition::Group {
            id: "genres".into(),
            name: format!("{} - Include", metadata.genre_title),
            filters: checkbox_filters(&values),
        });
        filters.push(FilterDefinition::Group {
            id: "notGenres".into(),
            name: format!("{} - Exclude", metadata.genre_title),
            filters: checkbox_filters(&values),
        });
    }
    append_select_definition(&mut filters, "type", &metadata.type_filter);
    append_select_definition(&mut filters, "chapter_count", &metadata.chapter_count);
    append_select_definition(&mut filters, "status", &metadata.status);
    append_select_definition(&mut filters, "sex", &metadata.gender);
    append_select_definition(&mut filters, "sort", &metadata.sort);
    filters
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterMetadata {
    pub genre_title: String,
    pub genres: Vec<OptionItem>,
    pub type_filter: Option<ParsedSelectFilter>,
    pub chapter_count: Option<ParsedSelectFilter>,
    pub status: Option<ParsedSelectFilter>,
    pub gender: Option<ParsedSelectFilter>,
    pub sort: Option<ParsedSelectFilter>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedSelectFilter {
    pub title: String,
    pub options: Vec<OptionItem>,
    pub default: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponseDto {
    pub list: Vec<SearchMangaDto>,
}

#[derive(Debug, Deserialize)]
pub struct SearchMangaDto {
    pub cover: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct PageListResponseDto {
    #[serde(default)]
    pub status: bool,
    #[serde(default)]
    pub msg: Option<String>,
    pub html: String,
}

fn parse_post_search_json(payload: &SearchResponseDto, base: &str) -> Result<Paged<CatalogItem>> {
    let entries = payload
        .list
        .iter()
        .map(|entry| {
            let url = canonical_url(base, &entry.url)?;
            let mut item = CatalogItem::new(path_key(base, &url)?, entry.name.clone());
            item.url = Some(url);
            item.cover = Some(absolute_url(base, &entry.cover)?.into());
            Ok(item)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Paged::new(entries, false))
}

fn parse_card(element: ElementRef<'_>, base: &str) -> Result<CatalogItem> {
    let link = select_all_element(element, "a")?
        .into_iter()
        .find(|candidate| {
            if text(*candidate).is_empty() {
                return false;
            }
            attr(*candidate, "href")
                .and_then(|href| absolute_url(base, &href).ok())
                .map(|href| is_manga_url(&href))
                .unwrap_or(false)
        })
        .ok_or_else(|| Error::new("Liliana list card has no manga link"))?;
    let url = canonical_url(
        base,
        &attr(link, "href").ok_or_else(|| Error::new("Liliana list card has no href"))?,
    )?;
    let title = text(link);
    if title.is_empty() {
        return Err(Error::new("Liliana list card has an empty title"));
    }
    let mut item = CatalogItem::new(path_key(base, &url)?, title);
    item.url = Some(url);
    item.cover = image_url(element, base)?.map(Into::into);
    Ok(item)
}

fn parse_chapter(element: ElementRef<'_>, base: &str) -> Result<MangaChapter> {
    let link = first_element_in(element, "a")
        .ok_or_else(|| Error::new("Liliana chapter entry has no link"))?;
    let url = canonical_url(
        base,
        &attr(link, "href").ok_or_else(|| Error::new("Liliana chapter entry has no href"))?,
    )?;
    let title = text(link);
    if title.is_empty() {
        return Err(Error::new("Liliana chapter entry has an empty title"));
    }
    let uploaded = first_element_in(element, "time[datetime]")
        .and_then(|time| attr(time, "datetime"))
        .and_then(|value| value.parse::<i64>().ok())
        .map(|seconds| seconds * 1000);
    Ok(MangaChapter {
        key: path_key(base, &url)?,
        title: Some(title.clone()),
        chapter_number: extract_number(&title),
        date_uploaded: uploaded,
        url: Some(url),
        ..MangaChapter::default()
    })
}

fn parse_page_image_url(element: ElementRef<'_>, base: &str) -> Result<Option<String>> {
    let Some(link) = first_element_in(element, "a") else {
        return Ok(None);
    };
    let Some(href) = attr(link, "href") else {
        return Ok(None);
    };
    let url = absolute_url(base, &href)?;
    if !is_page_image_url(&url) {
        return Ok(None);
    }
    Ok(Some(url))
}

fn page_from_url(index: usize, url: &str) -> Result<MangaPage> {
    let headers = image_headers(url)?;
    Ok(MangaPage {
        content: PageContent::Url {
            url: url.to_owned(),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    })
}

fn parse_status(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "ongoing" | "đang tiến hành" | "進行中" => "ongoing",
        "completed" | "hoàn thành" | "完了" => "completed",
        "on-hold" | "tạm ngưng" | "保留" => "on_hiatus",
        "canceled" | "đã huỷ" | "キャンセル" => "cancelled",
        _ => "unknown",
    }
}

fn append_select_definition(
    filters: &mut Vec<FilterDefinition>,
    id: &str,
    filter: &Option<ParsedSelectFilter>,
) {
    let Some(filter) = filter else { return };
    if filter.options.is_empty() {
        return;
    }
    filters.push(FilterDefinition::Select {
        id: id.to_owned(),
        name: filter.title.clone(),
        options: filter.options.clone(),
        default_index: filter
            .default
            .as_ref()
            .and_then(|default| {
                filter
                    .options
                    .iter()
                    .position(|option| &option.value == default)
            })
            .unwrap_or_default() as u32,
    });
}

fn checkbox_filters(values: &[(&str, &str)]) -> Vec<FilterDefinition> {
    values
        .iter()
        .map(|(label, value)| FilterDefinition::CheckBox {
            id: (*value).to_owned(),
            name: (*label).to_owned(),
            default: false,
        })
        .collect()
}

fn resolve_url(base: &str, language: &str, candidate: &str) -> Result<Option<UrlResolveResult>> {
    let base_url = Url::parse(base).map_err(url_error)?;
    let parsed = Url::parse(candidate).map_err(url_error)?;
    if parsed.host_str() != base_url.host_str() {
        return Ok(None);
    }

    let segments = parsed
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.len() < 2 || segments[0] != "manga" {
        return Ok(None);
    }

    let item_url = canonical_url(base, &format!("/manga/{}", segments[1]))?;
    let item = CatalogItem {
        key: path_key(base, &item_url)?,
        url: Some(item_url),
        language: Some(language.to_owned()),
        ..CatalogItem::default()
    };
    let mut result = UrlResolveResult {
        item: Some(item),
        ..UrlResolveResult::default()
    };
    if segments.len() > 2 {
        let chapter_url = canonical_url(base, candidate)?;
        let chapter = MangaChapter {
            key: path_key(base, &chapter_url)?,
            url: Some(chapter_url),
            language: Some(language.to_owned()),
            ..MangaChapter::default()
        };
        result.chapter_key = Some(chapter.key.clone());
        result.manga_chapter = Some(chapter);
    }
    Ok(Some(result))
}

fn parse_select_filter(
    document: &Html,
    select_id: &str,
    label_selector: &str,
) -> Result<Option<ParsedSelectFilter>> {
    let title_selector = selector(&format!(".select-div > label{label_selector}"))?;
    let option_selector = selector(&format!("#{select_id} > option"))?;
    let title = document
        .select(&title_selector)
        .next()
        .map(text)
        .unwrap_or_default();
    let options: Vec<OptionItem> = document
        .select(&option_selector)
        .filter_map(|element| {
            let value = element.value().attr("value")?.trim().to_owned();
            Some(option(value, text(element)))
        })
        .collect();
    if title.is_empty() && options.is_empty() {
        return Ok(None);
    }
    Ok(Some(ParsedSelectFilter {
        default: options.first().map(|option| option.value.clone()),
        title,
        options,
    }))
}

fn build_url<C: LilianaConfig>(
    path: &str,
    query: &[(&'static str, &'static str)],
) -> Result<String> {
    let mut url = Url::parse(&base_url::<C>()).map_err(url_error)?;
    url.set_path(path);
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

fn base_url<C: LilianaConfig>() -> String {
    C::BASE_URL.trim_end_matches('/').to_owned()
}

fn first_element<'a>(document: &'a Html, value: &str) -> Option<ElementRef<'a>> {
    selector(value)
        .ok()
        .and_then(|selector| document.select(&selector).next())
}

fn first_element_in<'a>(element: ElementRef<'a>, value: &str) -> Option<ElementRef<'a>> {
    selector(value)
        .ok()
        .and_then(|selector| element.select(&selector).next())
}

fn first_text(root: ElementRef<'_>, selector: &Selector) -> Option<String> {
    root.select(selector)
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

fn image_url(element: ElementRef<'_>, base: &str) -> Result<Option<String>> {
    let image = if element.value().name() == "img" {
        Some(element)
    } else {
        first_element_in(element, "img")
    };
    let Some(image) = image else {
        return Ok(None);
    };
    for key in ["data-lazy-src", "data-src", "data-cfsrc", "src"] {
        if let Some(value) = attr(image, key).filter(|value| !value.is_empty()) {
            return absolute_url(base, &value).map(Some);
        }
    }
    Ok(None)
}

fn image_headers(image_url: &str) -> Result<BTreeMap<String, String>> {
    Ok(BTreeMap::from([
        (
            String::from("Accept"),
            String::from("image/avif,image/webp,*/*"),
        ),
        (String::from("Host"), url_host(image_url)?),
    ]))
}

fn has_next_page(document: &Html) -> Result<bool> {
    let selector = selector(
        ".blog-pager a, .blog-pager > span.pagecurrent + span, a.nextpostslink, .page-numbers.next, nav.navigation a",
    )?;
    Ok(document.select(&selector).next().is_some())
}

fn is_manga_url(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            let segments: Vec<_> = url.path_segments()?.collect();
            Some(segments.len() == 2 && segments.first() == Some(&"manga"))
        })
        .unwrap_or(false)
}

fn is_page_image_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    !path.ends_with(".svg") && !lower.contains("loading_comments")
}

fn option(value: String, label: String) -> OptionItem {
    OptionItem { value, label }
}

fn select_all<'a>(document: &'a Html, value: &str) -> Result<Vec<ElementRef<'a>>> {
    Ok(document.select(&selector(value)?).collect())
}

fn select_all_element<'a>(element: ElementRef<'a>, value: &str) -> Result<Vec<ElementRef<'a>>> {
    Ok(element.select(&selector(value)?).collect())
}

fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}

fn text_list(document: &Html, value: &str) -> Result<Vec<String>> {
    Ok(document
        .select(&selector(value)?)
        .map(text)
        .filter(|value| !value.is_empty())
        .collect())
}

fn append_scalar_filter(pairs: &mut Vec<(String, String)>, filters: &Value, key: &str) {
    if let Some(value) = filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        pairs.push((key.to_owned(), value.to_owned()));
    }
}

fn append_csv_filter(pairs: &mut Vec<(String, String)>, filters: &Value, key: &str) {
    let Some(value) = filters.get(key) else {
        return;
    };
    match value {
        Value::Array(values) => {
            let selected: Vec<&str> = values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .collect();
            if !selected.is_empty() {
                pairs.push((key.to_owned(), selected.join(",")));
            }
        }
        Value::Object(values) => {
            let selected = values
                .iter()
                .filter_map(|(value, enabled)| enabled.as_bool().unwrap_or(false).then_some(value))
                .cloned()
                .collect::<Vec<_>>();
            if !selected.is_empty() {
                pairs.push((key.to_owned(), selected.join(",")));
            }
        }
        Value::String(value) if !value.is_empty() => {
            pairs.push((key.to_owned(), value.clone()));
        }
        _ => {}
    }
}

fn extract_number(value: &str) -> Option<f32> {
    let regex =
        Regex::new(r"(?i)(?:chapter|ch\.?|episode|ep\.?|volume|vol\.?|第)?\s*(-?\d+(?:\.\d+)?)")
            .expect("number regex should always compile");
    regex
        .captures(value)
        .and_then(|captures| captures.get(1))
        .and_then(|number| number.as_str().parse().ok())
}

fn url_host(url: &str) -> Result<String> {
    Url::parse(url)
        .map_err(url_error)?
        .host_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("URL has no host"))
}

fn json_error(error: serde_json::Error) -> Error {
    Error::new(error.to_string())
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_sdk::PageContent;
    use serde_json::json;

    struct TestConfig;

    impl LilianaConfig for TestConfig {
        const BASE_URL: &'static str = "https://raw1001.net";
    }

    #[test]
    fn builds_popular_latest_search_and_filter_urls() {
        assert_eq!(
            popular_url::<TestConfig>(2).unwrap(),
            "https://raw1001.net/ranking/week/2"
        );
        assert_eq!(
            latest_url::<TestConfig>(1).unwrap(),
            "https://raw1001.net/all-manga/1/?sort=last_update&status=0"
        );
        assert_eq!(
            search_url::<TestConfig>("カグラ", 3).unwrap(),
            "https://raw1001.net/search/3/?keyword=%E3%82%AB%E3%82%B0%E3%83%A9"
        );
        assert_eq!(
            filter_url::<TestConfig>(
                4,
                &json!({
                    "genres": ["2534", "2483"],
                    "notGenres": ["2535"],
                    "type": "manga",
                    "status": "on-going",
                    "sort": "views_week",
                    "chapter_count": "10",
                    "sex": "Boy"
                }),
            )
            .unwrap(),
            "https://raw1001.net/filter/4/?genres=2534%2C2483&notGenres=2535&type=manga&status=on-going&sort=views_week&chapter_count=10&sex=Boy"
        );
    }

    #[test]
    fn parses_catalog_cards_and_pagination() {
        let document = html::document(include_str!("../tests/fixtures/catalog.html"));
        let page = parse_catalog_html(&document, "https://raw1001.net/ranking/week/1").unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].title, "カグラバチ");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://raw1001.net/uploads/covers/kagurabachi.jpg")
        );
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_authors_tags_and_status() {
        let document = html::document(include_str!("../tests/fixtures/details.html"));
        let item = parse_details_html(&document, "https://raw1001.net/manga/kagurabachi").unwrap();
        assert_eq!(item.title, "カグラバチ");
        assert_eq!(item.authors, vec!["未詳"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(item.tags, vec!["アクション", "少年"]);
        assert_eq!(
            item.cover.as_ref().map(|request| request.url.as_str()),
            Some("https://raw1001.net/uploads/covers/kagurabachi.jpg")
        );
    }

    #[test]
    fn parses_chapters_and_unix_datetimes() {
        let document = html::document(include_str!("../tests/fixtures/chapters.html"));
        let chapters =
            parse_chapters_html(&document, "https://raw1001.net/manga/kagurabachi").unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].chapter_number, Some(125.0));
        assert_eq!(chapters[0].date_uploaded, Some(1_782_060_166_000));
        assert_eq!(
            chapters[0].url.as_deref(),
            Some("https://raw1001.net/manga/kagurabachi/di125hua")
        );
    }

    #[test]
    fn extracts_chapter_id_and_parses_sorted_ajax_pages() {
        let chapter = include_str!("../tests/fixtures/chapter.html");
        assert_eq!(extract_chapter_id(chapter).unwrap(), "351272");

        let payload: PageListResponseDto =
            serde_json::from_str(include_str!("../tests/fixtures/pages.json")).unwrap();
        assert!(payload.status);

        let document = html::document(&payload.html);
        let pages =
            parse_page_list_html(&document, "https://raw1001.net/manga/kagurabachi/di125hua")
                .unwrap();

        assert_eq!(pages.len(), 3);
        match &pages[0].content {
            PageContent::Url { url, context } => {
                assert_eq!(url, "https://sg.cdnkk.top/2026/06/22/12-49-37.webp");
                assert_eq!(context.as_ref().unwrap()["Host"], "sg.cdnkk.top");
                assert!(!context.as_ref().unwrap().contains_key("Referer"));
            }
            _ => panic!("expected URL page"),
        }
    }

    #[test]
    fn parses_live_filter_metadata_into_definitions() {
        let document = html::document(include_str!("../tests/fixtures/filters.html"));
        let metadata = parse_filter_metadata(&document).unwrap();

        assert_eq!(metadata.genre_title, "ジャンル");
        assert_eq!(metadata.genres[0], option("12".into(), "-BL-".into()));
        assert_eq!(
            metadata.chapter_count.as_ref().unwrap().options[1],
            option("10".into(), ">= 10".into())
        );
        assert_eq!(
            metadata.type_filter.as_ref().unwrap().options[2],
            option("manhua".into(), "Manhua".into())
        );

        let filters = build_filter_definitions(&metadata, TestConfig::filter_note());
        assert!(matches!(filters[0], FilterDefinition::Header { .. }));
        assert!(matches!(filters[2], FilterDefinition::Group { .. }));
    }

    #[test]
    fn rejects_malformed_details_and_missing_chapter_ids() {
        let details = html::document("<article class='a2'><header></header></article>");
        assert!(parse_details_html(&details, "https://raw1001.net/manga/test").is_err());
        assert!(extract_chapter_id("<script>const MANGA_ID = 1;</script>").is_err());
    }

    #[test]
    fn ignores_broken_images_and_plain_fallback_blocks_when_indexed_pages_exist() {
        let document = html::document(include_str!("../tests/fixtures/pages.json"));
        assert!(
            parse_page_list_html(&document, "https://raw1001.net/chapter")
                .unwrap()
                .is_empty()
        );

        let payload: PageListResponseDto =
            serde_json::from_str(include_str!("../tests/fixtures/pages.json")).unwrap();
        let ajax = html::document(&payload.html);
        let pages = parse_page_list_html(&ajax, "https://raw1001.net/chapter").unwrap();
        assert_eq!(pages.len(), 3);
    }

    #[test]
    fn maps_status_values_and_image_hosts_exactly_like_liliana() {
        assert_eq!(parse_status("進行中"), "ongoing");
        assert_eq!(parse_status("完了"), "completed");
        assert_eq!(parse_status("保留"), "on_hiatus");
        assert_eq!(parse_status("キャンセル"), "cancelled");
        assert_eq!(parse_status("mystery"), "unknown");

        let headers =
            image_headers("https://mgraw1111.wordpress.com/wp-content/uploads/test.jpg").unwrap();
        assert_eq!(headers["Host"], "mgraw1111.wordpress.com");
        assert!(!headers.contains_key("Referer"));
    }
}
