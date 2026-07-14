//! Madara manga-family support derived from `keiyoushi/extensions-source`.
//!
//! The upstream implementation is Apache-2.0. This crate preserves its
//! request and parsing behavior for Manatan component extensions.

use std::{collections::BTreeMap, marker::PhantomData};

use chrono::{DateTime, NaiveDate, Utc};
use manatan_sdk::{
    client::Client,
    host,
    html::{self, ElementRef, Html, Selector},
    CatalogItem, Error, FilterDefinition, MangaChapter, MangaPage, MangaSource, OptionItem,
    PageContent, Paged, Result,
};
use serde_json::{json, Value};
use url::Url;

/// Per-site values for the Madara manga family.
pub trait MadaraMangaConfig: 'static {
    const BASE_URL: &'static str;
    const USE_NEW_CHAPTER_ENDPOINT: bool = false;
    const FILTER_NON_MANGA_ITEMS: bool = true;
}

/// A configurable Madara source. Each leaf crate supplies its own config type.
pub struct MadaraMangaSource<C: MadaraMangaConfig> {
    client: Client,
    _config: PhantomData<C>,
}

impl<C: MadaraMangaConfig> Default for MadaraMangaSource<C> {
    fn default() -> Self {
        Self {
            client: Client::browser().header("Referer", format!("{}/", base_url::<C>())),
            _config: PhantomData,
        }
    }
}

impl<C: MadaraMangaConfig> MadaraMangaSource<C> {
    fn get_document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn post_document(&self, url: &str, form: &[(&str, &str)]) -> Result<(Html, String)> {
        let response = self
            .client
            .post(url)
            .header("X-Requested-With", "XMLHttpRequest")
            .form(form)
            .send()?
            .error_for_status()?;
        let final_url = response.final_url().to_owned();
        Ok((html::document(response.text()?), final_url))
    }

    fn item_url_for(&self, item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        absolute_url(C::BASE_URL, candidate)
    }
}

impl<C: MadaraMangaConfig> MangaSource for MadaraMangaSource<C> {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = listing_url::<C>(page, "views")?;
        let (document, final_url) = self.get_document(&url)?;
        parse_listing_html(&document, &final_url, C::FILTER_NON_MANGA_ITEMS)
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = listing_url::<C>(page, "latest")?;
        let (document, final_url) = self.get_document(&url)?;
        parse_listing_html(&document, &final_url, C::FILTER_NON_MANGA_ITEMS)
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let url = search_url::<C>(query, page, filters)?;
        let (document, final_url) = self.get_document(&url)?;
        parse_search_html(&document, &final_url)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = self.item_url_for(&item)?;
        let (document, final_url) = self.get_document(&url)?;
        let mut parsed = parse_details_html(&document, &final_url)?;
        parsed.key = item.key;
        parsed.url = Some(final_url);
        Ok(parsed)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let item_url = self.item_url_for(&item)?;
        let (document, final_url) = self.get_document(&item_url)?;
        let inline = parse_chapters_html(&document, &final_url)?;
        if !inline.is_empty() {
            return Ok(inline);
        }

        let holder = selector("div[id^='manga-chapters-holder']")?;
        let Some(holder) = document.select(&holder).next() else {
            return Ok(Vec::new());
        };
        let endpoint = chapters_endpoint_url(&final_url)?;
        let chapters_document = if C::USE_NEW_CHAPTER_ENDPOINT {
            self.post_document(&endpoint, &[])?
        } else {
            let manga_id = attribute(holder, "data-id").unwrap_or_default();
            self.post_document(
                &format!("{}/wp-admin/admin-ajax.php", base_url::<C>()),
                &[("action", "manga_get_chapters"), ("manga", &manga_id)],
            )?
        };
        parse_chapters_html(&chapters_document.0, &final_url)
    }

    fn pages(&mut self, _item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let chapter_url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (document, final_url) = self.get_document(chapter_url)?;
        parse_pages_html(&document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(madara_filters())
    }
}

fn base_url<C: MadaraMangaConfig>() -> String {
    C::BASE_URL.trim_end_matches('/').to_owned()
}

/// Build a regular Madara popular/latest URL, retaining the theme's page paths.
pub fn listing_url<C: MadaraMangaConfig>(page: u32, order: &str) -> Result<String> {
    let page = page.max(1);
    let path = if page == 1 {
        "/manga/".to_owned()
    } else {
        format!("/manga/page/{page}/")
    };
    let mut url = Url::parse(&base_url::<C>()).map_err(url_error)?;
    url.set_path(&path);
    url.query_pairs_mut().append_pair("m_orderby", order);
    Ok(url.to_string())
}

/// Build Madara's regular search endpoint from Manatan filter values.
pub fn search_url<C: MadaraMangaConfig>(query: &str, page: u32, filters: &Value) -> Result<String> {
    let page = page.max(1);
    let mut url = Url::parse(&base_url::<C>()).map_err(url_error)?;
    let path = if page == 1 {
        "/".to_owned()
    } else {
        format!("/page/{page}/")
    };
    url.set_path(&path);
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("s", query)
        .append_pair("post_type", "wp-manga");
    let mut filter_pairs = Vec::new();
    append_filter(&mut filter_pairs, filters, "author", "author", false);
    append_filter(&mut filter_pairs, filters, "artist", "artist", false);
    append_filter(&mut filter_pairs, filters, "year", "release", false);
    append_filter(&mut filter_pairs, filters, "order_by", "m_orderby", false);
    append_filter(&mut filter_pairs, filters, "adult", "adult", false);
    append_filter(&mut filter_pairs, filters, "genre_condition", "op", false);
    append_filter(&mut filter_pairs, filters, "status", "status[]", true);
    append_filter(&mut filter_pairs, filters, "genres", "genre[]", true);
    for (name, value) in filter_pairs {
        pairs.append_pair(&name, &value);
    }
    drop(pairs);
    Ok(url.to_string())
}

fn append_filter(
    pairs: &mut Vec<(String, String)>,
    filters: &Value,
    key: &str,
    parameter: &str,
    multiple: bool,
) {
    let Some(value) = filters.get(key) else {
        return;
    };
    match value {
        Value::String(value) if !value.is_empty() => {
            pairs.push((parameter.to_owned(), value.clone()));
        }
        Value::Array(values) if multiple => {
            for value in values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                pairs.push((parameter.to_owned(), value.to_owned()));
            }
        }
        _ => {}
    };
}

/// The current Madara endpoint used by newer themes to load chapters.
pub fn chapters_endpoint_url(manga_url: &str) -> Result<String> {
    let mut url = Url::parse(manga_url).map_err(url_error)?;
    let path = format!("{}/ajax/chapters", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(None);
    Ok(url.to_string())
}

pub fn parse_listing_html(
    document: &Html,
    base: &str,
    manga_only: bool,
) -> Result<Paged<CatalogItem>> {
    let entries = select_all(document, "div.page-item-detail, .manga__item")?
        .into_iter()
        .filter(|element| {
            !manga_only || has_class(*element, "manga") || has_class(*element, "manga__item")
        })
        .filter_map(|element| parse_card(element, base).ok())
        .collect();
    let next = selector("div.nav-previous, nav.navigation-ajax, a.nextpostslink")?;
    Ok(Paged::new(entries, document.select(&next).next().is_some()))
}

pub fn parse_search_html(document: &Html, base: &str) -> Result<Paged<CatalogItem>> {
    let entries = select_all(document, "div.c-tabs-item__content, .manga__item")?
        .into_iter()
        .filter_map(|element| parse_card(element, base).ok())
        .collect();
    let next = selector("div.nav-previous, nav.navigation-ajax, a.nextpostslink")?;
    Ok(Paged::new(entries, document.select(&next).next().is_some()))
}

fn parse_card(element: ElementRef<'_>, base: &str) -> Result<CatalogItem> {
    let link_selector = selector("div.post-title a, .post-title a, a")?;
    let link = element
        .select(&link_selector)
        .find(|candidate| candidate.value().attr("href").is_some() && !text(*candidate).is_empty())
        .ok_or_else(|| Error::new("Madara list entry has no title link"))?;
    let url = required_attribute(link, "href", "Madara list entry has no URL")?;
    let url = absolute_url(base, &url)?;
    let title = text(link);
    let mut item = CatalogItem::new(path_key(&url)?, title);
    item.url = Some(url);
    item.cover = image_url(element, base)?.map(Into::into);
    Ok(item)
}

pub fn parse_details_html(document: &Html, base: &str) -> Result<CatalogItem> {
    let title = first_text(
        document,
        "div.post-title h3, div.post-title h1, #manga-title > h1",
    )
    .ok_or_else(|| Error::new("Madara details page has no title"))?;
    let mut item = CatalogItem::new(path_key(base)?, title);
    item.url = Some(base.to_owned());
    item.authors = texts(document, "div.author-content > a, div.manga-authors > a");
    item.artists = texts(document, "div.artist-content > a");
    item.description = first_text(document, "div.description-summary div.summary__content, div.summary_content div.post-content_item > h5 + div, div.summary_content div.manga-excerpt");
    item.cover = first_element(document, "div.summary_image img")
        .map(|image| image_url(image, base))
        .transpose()?
        .flatten()
        .map(Into::into);
    item.tags = texts(document, "div.genres-content a, div.tags-content a");
    item.tags.sort();
    item.tags
        .dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    item.status = first_text(document, "div.summary-content").map(|value| json!(status(&value)));
    item.initialized = true;
    Ok(item)
}

pub fn parse_chapters_html(document: &Html, base: &str) -> Result<Vec<MangaChapter>> {
    select_all(document, "li.wp-manga-chapter")?
        .into_iter()
        .map(|element| parse_chapter(element, base))
        .collect()
}

fn parse_chapter(element: ElementRef<'_>, base: &str) -> Result<MangaChapter> {
    let link = element
        .select(&selector("a")?)
        .next()
        .ok_or_else(|| Error::new("Madara chapter has no link"))?;
    let raw_url = required_attribute(link, "href", "Madara chapter has no URL")?;
    let mut url = Url::parse(&absolute_url(base, &raw_url)?).map_err(url_error)?;
    url.set_query(Some("style=list"));
    let title = text(link);
    if title.is_empty() {
        return Err(Error::new("Madara chapter has an empty title"));
    }
    let date =
        first_text_element(element, "span.chapter-release-date").and_then(|date| parse_date(&date));
    Ok(MangaChapter {
        key: path_key(url.as_str())?,
        title: Some(title.clone()),
        chapter_number: chapter_number(&title),
        date_uploaded: date,
        url: Some(url.to_string()),
        ..MangaChapter::default()
    })
}

pub fn parse_pages_html(document: &Html, chapter_url: &str) -> Result<Vec<MangaPage>> {
    let selector = selector(
        "div.page-break img, li.blocks-gallery-item img, .reading-content .text-left img",
    )?;
    document
        .select(&selector)
        .filter_map(|image| image_url(image, chapter_url).transpose())
        .map(|url| {
            url.map(|url| MangaPage {
                content: PageContent::Url {
                    url,
                    context: Some(page_headers(chapter_url)),
                },
                headers: page_headers(chapter_url),
                ..MangaPage::default()
            })
        })
        .collect()
}

pub fn madara_filters() -> Vec<FilterDefinition> {
    vec![
        FilterDefinition::Text {
            id: "author".into(),
            name: "Author".into(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "artist".into(),
            name: "Artist".into(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "year".into(),
            name: "Year".into(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "status".into(),
            name: "Status slugs".into(),
            default: String::new(),
        },
        FilterDefinition::Select {
            id: "order_by".into(),
            name: "Order by".into(),
            default_index: 0,
            options: options(&[
                ("Relevance", ""),
                ("Latest", "latest"),
                ("A-Z", "alphabet"),
                ("Rating", "rating"),
                ("Trending", "trending"),
                ("Most views", "views"),
                ("New", "new-manga"),
            ]),
        },
        FilterDefinition::Select {
            id: "adult".into(),
            name: "Adult content".into(),
            default_index: 0,
            options: options(&[("All", ""), ("Hide", "0"), ("Only", "1")]),
        },
        FilterDefinition::Select {
            id: "genre_condition".into(),
            name: "Genre condition".into(),
            default_index: 0,
            options: options(&[("Any", ""), ("All", "1")]),
        },
        FilterDefinition::Text {
            id: "genres".into(),
            name: "Genre slugs".into(),
            default: String::new(),
        },
    ]
}

fn options(values: &[(&str, &str)]) -> Vec<OptionItem> {
    values
        .iter()
        .map(|(label, value)| OptionItem {
            label: (*label).into(),
            value: (*value).into(),
        })
        .collect()
}

fn image_url(element: ElementRef<'_>, base: &str) -> Result<Option<String>> {
    let image = if element.value().name() == "img" {
        Some(element)
    } else {
        element.select(&selector("img")?).next()
    };
    let Some(image) = image else { return Ok(None) };
    for name in [
        "data-src",
        "data-lazy-src",
        "data-cfsrc",
        "data-manga-src",
        "src",
    ] {
        if let Some(value) = attribute(image, name).filter(|value| !value.is_empty()) {
            return absolute_url(base, &value).map(Some);
        }
    }
    if let Some(srcset) = attribute(image, "srcset") {
        if let Some(value) = srcset
            .split(',')
            .filter_map(|entry| entry.split_whitespace().next())
            .last()
        {
            return absolute_url(base, value).map(Some);
        }
    }
    Ok(None)
}

fn page_headers(chapter_url: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(String::from("Referer"), chapter_url.to_owned())])
}

fn absolute_url(base: &str, candidate: &str) -> Result<String> {
    html::absolute_url(base, candidate)
}

fn path_key(url: &str) -> Result<String> {
    let url = Url::parse(url).map_err(url_error)?;
    let mut key = url.path().trim_end_matches('/').to_owned();
    if key.is_empty() {
        key.push('/');
    }
    Ok(key)
}

fn status(value: &str) -> &'static str {
    let value = value.to_ascii_lowercase();
    if value.contains("complete") || value.contains("finished") {
        "completed"
    } else if value.contains("on-going") || value.contains("ongoing") || value.contains("updating")
    {
        "ongoing"
    } else if value.contains("hold") || value.contains("hiatus") {
        "on_hiatus"
    } else if value.contains("cancel") {
        "cancelled"
    } else {
        "unknown"
    }
}

fn chapter_number(title: &str) -> Option<f32> {
    title.split_whitespace().find_map(|word| {
        word.trim_matches(|character: char| !character.is_ascii_digit() && character != '.')
            .parse::<f32>()
            .ok()
    })
}

fn parse_date(value: &str) -> Option<i64> {
    parse_absolute_date(value).or_else(|| parse_relative_date_at(value, host::now_millis()))
}

fn parse_absolute_date(value: &str) -> Option<i64> {
    let value = value.trim();
    for format in [
        "%B %d, %Y",
        "%b %d, %Y",
        "%Y-%m-%d",
        "%Y/%m/%d",
        "%Y年%m月%d日",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(value, format) {
            return date.and_hms_opt(0, 0, 0).map(|date| {
                DateTime::<Utc>::from_naive_utc_and_offset(date, Utc).timestamp_millis()
            });
        }
    }
    None
}

/// Parse the relative date vocabulary used across Madara themes against a
/// caller-supplied clock, which keeps fixtures deterministic.
pub fn parse_relative_date_at(value: &str, now_millis: i64) -> Option<i64> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.starts_with("today") {
        return Some(now_millis);
    }
    if normalized.starts_with("yesterday") {
        return Some(now_millis - 86_400_000);
    }
    let amount = normalized
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse::<i64>()
        .ok()?;
    let units = [
        (&["year", "years", "năm"][..], 365_i64 * 86_400_000),
        (&["month", "months", "tháng"][..], 30_i64 * 86_400_000),
        (&["week", "weeks", "tuần"][..], 7_i64 * 86_400_000),
        (
            &[
                "day",
                "days",
                "hari",
                "gün",
                "jour",
                "día",
                "dia",
                "วัน",
                "ngày",
                "giorni",
                "أيام",
                "天",
            ][..],
            86_400_000,
        ),
        (
            &[
                "hour",
                "hours",
                "jam",
                "saat",
                "heure",
                "hora",
                "ชั่วโมง",
                "giờ",
                "ore",
                "ساعة",
                "小时",
            ][..],
            3_600_000,
        ),
        (
            &[
                "minute",
                "minutes",
                "menit",
                "dakika",
                "min",
                "minuto",
                "นาที",
                "دقائق",
                "phút",
            ][..],
            60_000,
        ),
        (
            &[
                "second",
                "seconds",
                "detik",
                "saniye",
                "segundo",
                "วินาที",
                "ثوان",
                "giây",
            ][..],
            1_000,
        ),
    ];
    units
        .iter()
        .find(|(words, _)| words.iter().any(|word| normalized.contains(word)))
        .map(|(_, milliseconds)| now_millis - amount * milliseconds)
}

fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}
fn attribute(element: ElementRef<'_>, name: &str) -> Option<String> {
    html::attribute(element, name)
}
fn required_attribute(element: ElementRef<'_>, name: &str, error: &str) -> Result<String> {
    attribute(element, name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(error))
}
fn text(element: ElementRef<'_>) -> String {
    html::text(element)
}
fn first_element<'a>(document: &'a Html, value: &str) -> Option<ElementRef<'a>> {
    selector(value)
        .ok()
        .and_then(|selector| document.select(&selector).next())
}
fn first_text(document: &Html, value: &str) -> Option<String> {
    first_element(document, value)
        .map(text)
        .filter(|value| !value.is_empty())
}
fn first_text_element(element: ElementRef<'_>, value: &str) -> Option<String> {
    selector(value)
        .ok()
        .and_then(|selector| element.select(&selector).next())
        .map(text)
        .filter(|value| !value.is_empty())
}
fn texts(document: &Html, value: &str) -> Vec<String> {
    selector(value)
        .map(|selector| {
            document
                .select(&selector)
                .map(text)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}
fn select_all<'a>(document: &'a Html, value: &str) -> Result<Vec<ElementRef<'a>>> {
    Ok(document.select(&selector(value)?).collect())
}
fn has_class(element: ElementRef<'_>, class: &str) -> bool {
    element.value().classes().any(|value| value == class)
}
fn url_error(error: url::ParseError) -> Error {
    Error::new(error.to_string())
}
